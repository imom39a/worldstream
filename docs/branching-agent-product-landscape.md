# Branching agent systems: implementation landscape

Date: 2026-09-13. Research note, not an accepted architecture decision or implementation claim.

## Finding

Branching alone is already available in agent execution frameworks, databases, and coding harnesses. The more specific WorldStream hypothesis is **independently hosted participants choosing durable alternative Room lineages, contributing under scoped authority, and promoting precisely identified results under Activity Pack rules**. This combination is a product hypothesis to validate; this review does not establish market absence or defensible uniqueness.

The existing [domain model](../CONTEXT.md) defines one authoritative situation per Room and an immutable Pack Revision for its whole lineage. [ADR 0001](adr/0001-product-boundary.md) explicitly excludes timeline forks and branch promotion from the frozen releases. Consequently, this note does not redefine a Room or add a new canonical entity called a world. A future branch design needs an explicit ADR that preserves or deliberately revises those boundaries.

## What is already concrete elsewhere

### LangGraph and LangSmith Agent Server

LangGraph supports resuming from a checkpoint and creating an alternative continuation through `update_state`. The original history remains. Subsequent nodes execute again, including model calls, API requests, and interrupts. Thus this is useful execution branching, but a checkpoint does not establish rollback or isolation of a database, filesystem, or remote service touched by those nodes. Subgraphs need their own checkpointer for internal time-travel granularity. [Official time-travel guide](https://docs.langchain.com/oss/python/langgraph/use-time-travel)

Agent Server also exposes `threads.copy`, which creates an independent thread with the original thread's history at copy time. Its SDK can search threads by status and custom metadata and read thread state/history. An application can therefore expose discoverable alternatives to agents; saying other products only provide a private developer debugger would be inaccurate. The reviewed API does not itself define a shared participant electorate, an Activity Pack validator, or a cross-thread promotion receipt. These can be application code. [Official thread API guide](https://docs.langchain.com/langsmith/use-threads)

For durability, LangGraph distinguishes synchronous, asynchronous, and exit-only checkpoint persistence. Default checkpoints write complete state-channel values at each super-step; its beta `DeltaChannel` reduces storage for accumulating channels. These are useful implementation precedents, not evidence that a particular 100,000-turn workload has been qualified. [Checkpointers and durability modes](https://docs.langchain.com/oss/python/langgraph/checkpointers)

### Dolt

Dolt provides database branches that multiple SQL clients can select and use concurrently, with branch-local visibility and `REPEATABLE_READ` transactions. Clients can connect to a branch or a read-only commit. This is a direct precedent for several actors sharing each alternative, rather than assigning one private alternative to each actor. The documented server cannot commit writes to multiple branches in one transaction. [Branch and revision semantics](https://www.dolthub.com/docs/sql-reference/version-control/branches/)

Dolt's merge operates on database structure and data cells, and a merge commit has two parents; a fast-forward is possible when the destination has not diverged. This makes lineage and the unit of conflict explicit. It does not prove an arbitrary application invariant merely because data cells merge cleanly. [Merge model](https://www.dolthub.com/docs/concepts/dolt/git/merge/)

The SQL merge interface reports both merge conflicts and constraint violations. It supports resolving or aborting a merge before committing the transaction. WorldStream should borrow this distinction: a syntactically combinable result can still violate Pack rules. Voting must not bypass either class of failure. [Merge conflict and constraint handling](https://www.dolthub.com/docs/sql-reference/version-control/merges/)

### Claude Code and other coding harnesses

Claude Code supports parallel sessions and subagents in separate Git worktrees. Its current documentation also describes checks against edits and commands redirected into the main checkout. Worktrees nevertheless share repository metadata, project plugins, and some saved approvals; they are a specific file-edit isolation mechanism. [Claude Code worktree documentation](https://code.claude.com/docs/en/worktrees)

Claude Code sessions can discover peers through `ListAgents` and communicate through `SendMessage`, including independently started sessions under documented account, platform, and connection conditions. This directly overlaps the proposed experience of agents seeing other ongoing work and sharing decisions. The channel carries plain text; durable typed ballots and promotion rules are not specified by this messaging contract. That finding is about this contract, not a claim that such rules cannot be built on top. [Cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)

VS Code separately documents that worktree isolation does not restrict commands or network access. A worktree therefore cannot be treated as a clone of the entire world an agent interacts with. Branching a code checkout needs additional environment control if experiments modify shared services. [Agent harness and isolation guide](https://code.visualstudio.com/docs/agents/run/agent-harnesses)

### Temporal and Restate

Temporal's `ResetWorkflowExecution` terminates the current execution and restarts from a selected historical point. That operation is a recovery mechanism, not concurrent preservation of two live alternatives under one workflow identity. A developer could model alternatives as separate workflows and implement their coordination. [Temporal service API](https://github.com/temporalio/api/blob/main/temporal/api/workflowservice/v1/service.proto)

Restate journals non-deterministic step results and uses them during retries; it also documents compensation patterns for external effects. This is evidence for durable execution and recorded outcomes, not a general ability to undo the external world. [Restate durable steps](https://docs.restate.dev/develop/ts/durable-steps)

Restate's implementation keeps materialized state, journals, and timer indices near partition-local ordered logs and recovers from snapshots plus a suffix. This is relevant to WorldStream's long-history recovery work independently of whether alternatives are added. Its distributed architecture should not be interpreted as permission to expand WorldStream's currently accepted single-process scope. [Restate architecture](https://docs.restate.dev/references/architecture)

### Direct counter-evidence: STORM

Liu et al.'s *Multi-agent Collaboration with State Management*, arXiv v1 dated 2026-05-19, studies a shared workspace with optimistic concurrency checks on agents' file read sets. A manager assigns tasks and commits; engineers use intent annotations. With Sonnet, reported Commit0-Lite macro scores are 82.5 versus 63.8 for GitWorktree; PaperBench Code-Dev scores are 74.1 versus 72.7. The latter uses an LLM judge without running experiments. Scaling reaches at most eight engineers. “Combined” scores select the better single/multi-agent result per task, not a deployable selection policy. The paper acknowledges terminal-write bypass, uncoordinated commands, false conflicts from file-level tracking, and semantic incompatibility despite accepted writes. It does not establish 100-agent, day-long durability. [Versioned full text, especially sections 2–3 and appendices D–E](https://arxiv.org/html/2605.20563v1)

The linked repository exists. At inspected commit `4914a44de2d4b369cc78fe7da8164b605a67db1c`, the runner tags `file_editor` with an agent identifier, but the inspected executor does not expose the paper's read-set validation. This limited inspection does not establish an executable reproduction; no benchmark was run. [Runner source](https://github.com/dreamyang-liu/STORM/blob/4914a44de2d4b369cc78fe7da8164b605a67db1c/STORM/core/subagent.py), [editor executor source](https://github.com/dreamyang-liu/STORM/blob/4914a44de2d4b369cc78fe7da8164b605a67db1c/software-agent-sdk/openhands-tools/openhands/tools/file_editor/impl.py)

Our inference: this materially narrows the proposed advantage. Complementary, coupled work may benefit from shared current state and early checks; preserve branches for competing hypotheses or experiments whose separation creates value. The benchmark must include both approaches. The paper's manager-controlled single workspace does not evaluate participant-selected alternative lineages with voting and validated promotion.

## Comparison for our proposed experience

“Documented” means supported by the sources above. “Custom” means application code would need to define the proposed semantics. “Unverified” means this review did not establish that property; it is not a claim of absence.

| Capability | LangGraph / Agent Server | Dolt | Claude Code worktrees | Temporal / Restate |
|---|---|---|---|---|
| Preserve divergent continuations | Documented checkpoints and independent thread copies | Documented database branches | Documented separate code branches/worktrees | Temporal reset replaces current run; concurrent alternatives custom |
| Several independent actors inspect/select alternatives | Thread search/read API documented; agent-facing policy custom | Branch-selecting SQL clients documented | Peer discovery/messaging documented; joining another session's work policy custom | Discovery and work allocation for alternatives custom |
| Isolation boundary | Checkpointed graph state | Versioned database state | Tracked file workspace with shared repository facilities | Journaled workflow/invocation state |
| Generic rollback of external effects | Unverified; future API calls execute again | Outside database boundary unverified | Outside file workspace unverified | Requires application effect policy/compensation |
| Automatic combination | Application reducers; cross-thread promotion custom | Documented structural/data merge | Git merge plus harness/user workflow | Application-specific |
| Durable votes over exact alternative revisions | Custom | Can store ballots, policy custom | Typed durable ballot contract unverified | Can implement a voting workflow, policy custom |
| Domain-valid promotion with current authority checks | Custom | SQL constraints help; application invariants custom | Code validation/review workflow custom | Application-specific |
| Our 100 agents / 100,000+ turns / one day target | Unverified | Unverified | Unverified | Unverified |

## Consequences for product research

The following are our inferences and proposed experiments, rather than claims made by the sources.

1. **Do not position the feature as “agents can branch.”** Compare against an application built with LangGraph thread copying and discovery, a database with branches, and existing worktree-based agent cooperation. The value must survive those comparisons.
2. **Test collective participation within an alternative.** Three agents should be able to join the same branch, get scoped current state, and make independent contributions, while other agents work elsewhere. This tests more than a supervisor launching a private child task.
3. **Make the result, evidence, and electorate explicit.** A vote should name an immutable branch result and validation evidence. Later branch changes should not silently inherit that endorsement. The application must define who may vote and what the vote authorizes.
4. **Start with selection and validated import.** Select one solved candidate or import a bounded artifact whose preconditions still hold. Generic merging of arbitrary Room states is a much larger promise and often the wrong operation.
5. **Demonstrate observation and continuity.** A new external Runner should discover allowed alternatives and join with bounded current state, without replaying every competing transcript. An observer should see the exact evidence behind promotion.
6. **Measure useful work, not branch count.** Compare one shared path, independent best-of-N attempts, and collaborative branches at equal model/token budgets. Record verified success, time to success, wasted work, invalid promotion attempts, and context/storage/recovery costs.

Hanoi is a controlled first probe of long dependent sequences and traceability. It is a weaker test of complementary branch merging: alternative trajectories usually represent competing board states, so selecting a valid path is more natural than combining two boards. A second Pack should exercise independently produced artifacts or constraints that can actually be combined, and should expose cases where each artifact passes alone but their combination fails.

The product question worth testing is: **Do developers need a standard participation and validation contract across durable alternatives enough that building it themselves on these primitives is costly?** Two outside applications needing the same missing semantics would provide stronger direction than the multiverse analogy alone, and match the revisit discipline in ADR 0001.

## Limits of this review

This review covers official documentation, API source, and one directly relevant primary preprint retrieved on 2026-09-13. It includes no executed benchmark, full code audit of these systems, market census, or claim about closed-source internals. Documentation can evolve; pin source and dependency revisions before implementing an interoperability prototype. “Replay” also differs by system: WorldStream Replay reconstructs recorded history, whereas LangGraph time-travel replay deliberately executes the future again.
