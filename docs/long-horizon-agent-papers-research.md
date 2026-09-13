# Long-horizon agent research and implications for WorldStream

Status: Non-normative research, 2026-09-12. This note does not change accepted
ADRs, wire contracts, or the Activity Pack contract. It reviews eight primary
papers and selected author-maintained implementations. Results below are the
authors' reported results; no paper benchmark was reproduced in this task.
Dates identify initial publication, with reviewed versions stated where useful.
This is a focused selection, not an exhaustive literature review.

The practical conclusion is that bounded model context, retained evidence,
explicit work state, and local verification are well-supported directions.
The reviewed evidence does not establish reliable 100,000-step open-world,
concurrent, real-time participation. Memory capacity, successful reasoning
horizon, runtime durability, and response latency are separate properties.

## Papers and actual evidence

### 1. MemGPT: Towards LLMs as Operating Systems

Packer et al., 2023-10-12; reviewed paper v2. MemGPT gives a finite-context
model tools for moving information between working context, a recent-message
queue, and external recall/archival stores. Memory-pressure warnings,
summarization, paginated retrieval, and event-triggered execution control the
loop. The multi-session experiment uses five sessions of approximately twelve
messages each, followed by a memory question. GPT-4 with MemGPT reaches 92.5%
deep-memory-retrieval accuracy versus 32.1% for the paper's fixed-context
baseline. This supports explicit memory management, not an experimentally
demonstrated unlimited reliable session. [Paper, sections 2–3](https://arxiv.org/html/2310.08560v2).

WorldStream implication: an external Runner can implement this kind of
Agent-Private Memory. It must remain distinct from Authoritative Room State.
An Invocation should receive bounded authorized information and retrieve
additional evidence selectively; the model's memory edits should not silently
rewrite accepted Room facts.

### 2. LongMemEval: Benchmarking Chat Assistants on Long-Term Interactive Memory

Wu et al., 2024-10-14; ICLR 2025. The benchmark has 500 questions testing
extraction, reasoning across sessions, temporal reasoning, knowledge updates,
and abstention. Its standard histories are approximately 115k and 1.5M tokens.
In the paper's GPT-4o experiment with Chain-of-Note, accuracy is 92.4% with
oracle evidence sessions and 64.0% with the full 115k-token history. These are
historical model measurements, not a claim about current products. The work
also studies indexing, retrieval, and reading separately. It is primarily
question answering over compiled histories, not execution against a changing
external system. [Paper, sections 3–4](https://arxiv.org/html/2410.10813v2).

WorldStream implication: test whether a fresh Invocation can identify the
latest valid fact, superseded facts, relevant time, and missing evidence.
Measure retrieval recall separately from answer correctness. A bigger prompt
is not sufficient evidence of better continuity.

The [official repository](https://github.com/xiaowu0162/LongMemEval) provides
evaluation and memory-pipeline code and identifies a September 2025 cleaned
dataset. Pin the dataset revision when comparing results.

### 3. ReSum: Unlocking Long-Horizon Search Intelligence via Context Summarization

Wu et al., 2025-09-16; reviewed v1. ReSum periodically replaces a growing
search trajectory with the original question and a compact summary. Its
training variant adapts agents to reasoning after summary boundaries. The
paper reports a 4.5 percentage-point average improvement over ReAct across its
benchmarks. Crucially, evaluation has a maximum of **60 tool calls per query**
and uses 32k-context WebSailor models. Its unbounded-exploration language
describes removal of a mechanical context limit, not demonstrated arbitrary
task reliability. [Paper, sections 3–4 and appendix C](https://arxiv.org/html/2509.13313v1).

WorldStream implication: summary boundaries can replace Invocations without
ending logical work. Preserve exact open obligations and evidence references
outside a lossy summary. Train or evaluate continuation from compressed
context explicitly, rather than assuming an agent that handles full history
also handles a summary well.

The paper points to the authors' [DeepResearch repository](https://github.com/Alibaba-NLP/DeepResearch).
That shared repository link alone does not establish an exact, pinned
reproduction of every ReSum experiment.

### 4. AgentFold: Long-Horizon Web Agents with Proactive Context Management

Ye et al., 2025-10-28; reviewed arXiv v1. AgentFold learns to retain recent
interaction details while condensing older trajectory blocks at different
scales. Its 30B-A3B model reports 36.2% on BrowseComp and 47.3% on
BrowseComp-ZH. Accuracy scaling is tested with turn limits up to **256**. A
separate experiment extends the limit to **500 turns**, with context mostly
below 20k tokens. The paper does not establish 500 correct dependent actions,
much less 100,000; the context-growth trace and task-success measurements are
different evidence. [Paper, sections 3–4](https://arxiv.org/html/2510.24699v1).

WorldStream implication: use summaries with explicit covered ranges and
retain source references. Current authorized state, the most recent outcome,
and historical navigation aids serve different needs. This is a promising
Runner strategy, not a replacement for canonical persistence or Pack rules.

The [ICLR 2026 paper](https://openreview.net/pdf?id=IuZoTgsUws) links the
authors' shared DeepResearch repository and released model artifact.

### 5. MAKER: Solving a Million-Step LLM Task with Zero Errors

Meyerson et al., 2025-11-12; reviewed v1. MAKER reports a correct 20-disk
Towers-of-Hanoi sequence: **1,048,575 dependent moves**. It decomposes work
into single moves, samples multiple proposed results, uses a first-to-ahead
voting margin, and discards responses with warning signs such as malformed
output. The large run uses GPT-4.1-mini and a margin of three. Its theory
requires favorable per-step sampling probabilities and sufficiently
decorrelated errors; the decomposition and known algorithm are supplied.
This is strong evidence for narrowly specified, locally checkable execution,
not autonomous discovery of a million-step real-world plan. [Paper,
sections 3–5](https://arxiv.org/html/2511.09030v1).

WorldStream implication: make each accepted contribution small and
checkable. Use deterministic validation whenever available; reserve extra
model sampling for uncertain judgments. Votes are evidence about a proposal,
not transaction consensus or proof of external truth.

The [author repository](https://github.com/cognizant-ai-lab/neuro-san-benchmarking)
identifies original code for appendix F at commit `a7a22f8` and a Hanoi
playground. This should not be described as an independently reproduced
million-step trace.

### 6. BEAM and LIGHT: Beyond a Million Tokens

Tavakoli et al., 2025-10-31; reviewed v2, revised 2026-02-21; ICLR 2026.
BEAM provides 100 synthetic conversations and 2,000 validated questions,
including a 10M-token tier. Its table reports approximately 7,757 dialogue
turns per 10M-token chat. LIGHT combines episodic retrieval, recent working
memory, and a scratchpad. In the GPT-4.1-nano indexing experiment, LIGHT's
10M-token aggregate score is 0.226 with exact inner-product indexing and
0.237 with HNSW. This demonstrates a substantial remaining quality gap;
10M retained tokens do not imply reliable recall or action execution.
[Paper, tables 3 and 12](https://arxiv.org/html/2510.27246v2).

WorldStream implication: evaluate memory beyond the model window and report
specific failure categories, including updates, temporal ordering, and
contradictions. Keep benchmark scores distinct from percentages of valid
Room Transitions or completed obligations.

The authors' [LIGHT implementation](https://raw.githubusercontent.com/mohammadtavakoli78/BEAM/main/src/answer_probing_questions/light.py)
exposes its memory construction and retrieval pipeline. It is research code
with whole-input processing and character-based token estimates. Reusing its
ideas requires separate engineering for bounded allocation, exact budgets,
incremental ingestion, and recovery.

### 7. LongMemEval-V2: Evaluating Long-Term Agent Memory Toward Experienced Colleagues

Wu et al., 2026-05-12; reviewed v1 preprint. This benchmark contains 451
questions over web-agent histories with up to approximately 500 trajectories
and 115M multimodal tokens. It measures context gathering via insertion and
query APIs, followed by a fixed reader with a 200k-token limit. AgentRunbook-C
reports 74.9%/70.1% accuracy and 108.3s/139.9s query latency on Small/Medium.
The large histories comprise many trajectories, not one continuously
successful task. This is evidence for an accuracy/latency tradeoff in
experience retrieval, not a low-latency real-time control loop.
[Paper, sections 3–5](https://arxiv.org/html/2605.12493v1).

WorldStream implication: directly provide current typed facts and Action
Offers instead of requiring an agent to infer them from large historical
traces. Reserve expensive historical investigation for cases that need it.
Always measure freshness and useful-action latency alongside retrieval quality.

The [actual Memory interface](https://raw.githubusercontent.com/xiaowu0162/LongMemEval-V2/main/memory_modules/memory.py)
has `insert`, `query`, and persistence hooks. The
[AgentRunbook-R implementation](https://raw.githubusercontent.com/xiaowu0162/LongMemEval-V2/main/memory_modules/agentrunbook_r.py)
separates raw-state evidence, change descriptions, and procedural notes.
These are useful reference interfaces for a Runner memory adapter, not Room
authority. Active follow-up work includes an
[August 2026 AgentRunbook-C V2 release](https://xiaowu0162.github.io/longmemeval-v2/agentrunbook-c-v2/).

### 8. Recursive Language Models

Zhang, Kraska, and Khattab, 2025-12-31; reviewed v3, revised 2026-05-11.
RLMs place a large prompt in an external programming environment and let the
model inspect it and invoke sub-models on programmatically selected pieces.
Intermediate values remain outside the root model context. On BrowseComp+
with 1,000 documents and 6M–11M input tokens, GPT-5 RLM at recursion depth
one reports 91.3% accuracy versus 70.5% for the compaction baseline. The
experiments include corpus QA, aggregation, code QA, and pairwise reasoning.
This is large-input inference, not a durable 100,000-turn session; it neither
establishes real-time bounds nor guarantees correct selective reading.
[Paper, sections 2–4](https://arxiv.org/html/2512.24601v3).

WorldStream implication: make retained authorized evidence addressable by
stable handles, with bounded slices and explicit provenance. An external
Runner can orchestrate deeper analysis without loading everything into one
Invocation or placing model execution in the reducer. Recursive work still
needs explicit budgets and cancellation.

The authors publish an [RLM inference library](https://github.com/alexzhang13/rlm)
supporting multiple execution environments. Its availability supports
experimentation; it is not evidence of WorldStream-compatible durability,
authority, or recovery semantics.

One additional measurement reference is METR's
[task-completion time horizon](https://metr.org/time-horizons/), derived from
its [2025 paper](https://arxiv.org/abs/2503.14499). The horizon measures the
human-expert task duration at which an agent is predicted to succeed with a
specified probability. It does **not** measure how long the agent runs
continuously. The current methodology page flags estimates above sixteen
hours as unreliable with its task suite. Use the metric for capability
evaluation, not as a runtime-uptime guarantee.

## First-principles deductions for our primitives

The following are design deductions, not additional claims established by
the papers.

**A retained transcript and a current work state answer different questions.**
History answers what happened and why a claim was made. Activity State answers
what is accepted now and what remains unresolved. Invocation Context supplies
the authorized subset needed for the next decision. A model summary is a
fallible index into evidence, not an authoritative checkpoint.

**Finite context cannot preserve arbitrary history exactly.** If there are
more distinct histories than possible bounded summaries, at least two
histories map to the same summary. A future question can distinguish them.
Therefore no fixed-size summary can preserve every possible answer about an
unbounded past. The application must specify a sufficient set of current
facts for ordinary decisions and preserve retrievable evidence for other
questions. Bounded active work also requires admission limits; it cannot be
achieved by silently forgetting unresolved obligations.

**Reliability needs containment, not just more turns.** In an illustrative
model where each required step succeeds independently with probability
`1 - p`, all `N` succeed with probability `(1 - p)^N`. At `p = 0.001` and
`N = 100,000`, that is about `3.54e-44`. To obtain 99% probability of no
errors under these assumptions requires `p` around `1.0e-7`. Real agent
errors are often correlated, and real tasks can sometimes recover, so this
is not a universal performance prediction. It exposes why long-running
systems need local validation, recoverable progress, and explicit handling
of uncertainty. Retrying a durable operation prevents duplication; it does
not correct a confidently wrong judgment.

**Real-time participation adds freshness to memory.** A perfectly recalled
observation can already be stale. An accepted assessment should preserve its
author, source identities and versions, work revision, evidence references,
and disposition under Pack rules. New evidence can invalidate an old claim
without deleting the claim's provenance. The current strict Room-head check
continues to govern admission; dependency-based admission would be a separate
protocol proposal.

**Verification belongs near the accepted change.** Let a policy propose a
typed Action. Validate schema, authority, current basis, domain preconditions,
and invariants before commitment. A syntactically valid proposal is not
necessarily useful or true. The Pack can accept that a Membership made an
assessment without asserting that the assessment has been externally
verified. External effects need separately recorded identities and confirmed
outcomes; model agreement cannot supply a transaction guarantee.

These deductions preserve the existing [bounded Invocation and lease
boundary](adr/0009-activation-intents-context-and-lease-fencing.md) and
[bounded Companion Plan decision](adr/0028-record-bounded-companion-plans-in-activity-state.md).
The proposed runtime qualification work is described in
[Long-running Room primitives and qualification proposal](long-running-room-primitives-proposal.md).

## A useful experiment before changing core semantics

Use one nonterminal Room with exact current source versions, a bounded set
of unresolved obligations, and attributed assessments. Compare three Runner
context strategies with the same model and budgets: recent history,
summary plus retrieval, and current Projection plus explicit obligations
plus retrieval. Replace Invocations repeatedly; change sources while a
Runner is offline; introduce conflicting evidence and delayed Action replies.

Measure runtime properties separately from policy quality. Runtime evidence
includes accepted-Action integrity, duplication, current-state reconstruction,
memory usage, backlog, and latency at increasing history sizes. Policy
evidence includes missed obligations, obsolete claims reused as current,
unsupported completion, wrong evidence, failure to abstain, and time to an
accepted useful contribution. A deterministic Runner qualifies the machinery;
bounded model experiments qualify continuation quality. A 100,000-Transition
soak cannot by itself prove 100,000 correct model decisions.
