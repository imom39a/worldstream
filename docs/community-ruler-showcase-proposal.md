# Community solver proposal: the shortest Golomb ruler

Date: 2026-09-14. Status: proposed research activity; no production Pack, Runner, kernel, or deployment change. The small worked example below was checked locally; it was not produced by a live multi-agent Room.

## Selected problem

Place a fixed number of marks at distinct integer positions on a ruler. Every pair of marks must have a different distance. Find the shortest possible ruler, and establish why a shorter one cannot work. This is the Golomb ruler problem. [CSPLib problem 006](https://www.csplib.org/Problems/prob006/).

Recommend this as the next community-solver experiment after Hanoi. It offers a compact construction, exact integer rules, partial progress, independently explorable search regions, and a final mathematical result combining a construction with an impossibility argument. The Activity Pack needs the problem specification and accepted evidence rules, not a stored winning ruler.

This selection follows comparison with [circle packing](circle-packing-community-candidate.md), [no-three-in-line and Ramsey coloring](discrete-math-community-candidate.md), and the [detailed Golomb assessment](golomb-ruler-community-candidate.md). Circle packing has attractive motion but introduces numerical optimization and certification choices. No-three-in-line has a stronger board display and more direct combination of partial constructions. The ruler problem wins here because construction and proof can make two different kinds of community contribution visible in one result. This is a design judgment, not an experimental finding.

## Why this is a real community research problem

The distributed.net OGR-28 project announced completion on November 23, 2022 after about 8.5 years. More than 65,000 volunteers contributed. The project exhaustively checked its search regions and established that the previously known 28-mark ruler of length 585 was optimal. This demonstrates distributed mathematical work at scale. It does not demonstrate LLM reasoning benefits or qualify WorldStream's capacity. [First-party completion report](https://blogs.distributed.net/2022/11/23/03/28/bovine/).

We should start much smaller. The proposal is a known-problem systems experiment, not a claim to discover a new mathematical record. A published upper bound is a useful reference; finding a ruler with that length does not independently prove optimality.

## An exact five-mark explanation

These illustrative contributions were verified on 2026-09-14 using a small local Python enumeration, outside WorldStream:

| Contribution | Established fact |
| --- | --- |
| A finds marks `0,1,4,6,13` | Ten unique pair distances; a valid ruler of length 13 exists. |
| B finds marks `0,1,4,9,11` | Ten unique pair distances; a valid ruler of length 11 exists. |
| C excludes all five-mark rulers of length at most 10 | With the first mark normalized to zero, all `choose(10,4)=210` possible remaining mark sets were enumerated; none was valid. |
| Combine B and C | The minimum length is exactly 11. |

Initially, counting the ten different positive integer distances proves only that length must be at least ten. B reduces the construction bound from thirteen to eleven. C raises the proved lower bound from ten to eleven. Both bounds then agree.

The enumeration covers every nonnegative integer ruler of length at most ten after translation: choose the other four marks from positions 1 through 10. It does not rely on an unproved symmetry reduction or search heuristic. A heuristic timeout would not establish the same result.

## How the Room would work

Use one proposed **Golomb Ruler Activity Pack**. A Room configuration fixes mark count, integer-position rules, any allowed-position constraints, time budget, and verification policy. For an accessible first live trial, use eight marks, then calibrate eight-to-twelve-mark tasks for research. Do not promise a one-day completion time before measuring the external solver and proof work.

Activity State contains bounded current work: best valid construction, proved lower bound, active approaches, retained evidence references, current search regions, and requested help. Large search logs and certificates remain referenced Artifacts. Historical canonical facts remain retained according to current guarantees; a small active view does not authorize silently deleting their evidence.

Participants contribute to the process:

- Propose a different construction strategy or a prefix of mark positions.
- Use a bounded external search tool to extend, repair, or shorten a candidate.
- Derive a necessary condition that can rule out some placements.
- Split a search region into exact, collectively exhaustive child regions.
- Investigate one region and submit an auditable result.
- Reuse another Participant's construction, proven bound, or applicable exclusion result.
- Ask for help after a blocked approach and continue promising alternatives.

An illustrative cycle is: a Participant proposes a family of rulers; another finds a shorter valid member; an accepted improvement narrows the lengths that remain worth searching; other Participants receive Attention Signals; their next Invocations explore unresolved shorter lengths; verified exclusion evidence raises the lower bound. When a valid construction and a lower-bound proof meet, the Pack can establish the optimal Outcome.

The Pack defines coordination, admissible evidence, and progress. Runners perform model work and use external mathematical tools. Agents need not make an LLM call for every trial placement. Candidate validation is a small integer calculation; exhaustive search can be expensive and belongs outside the Pack's bounded deterministic callbacks.

## Useful integration and explicit conflicts

There are two different graphs. An approach graph records alternatives and reused ideas. A coverage graph records how a precise finite search domain was partitioned and which parts have been excluded. Proving optimality requires complete valid coverage of the relevant domain, not a popular approach or an impressive number of explored nodes.

Do not claim arbitrary partial rulers can merge. Cross-distances can introduce duplicates even when both pieces are individually valid. A combined construction is a new candidate that must satisfy every pair constraint.

An improved construction can safely reduce the shared upper bound when it is valid for the exact Room problem. An exclusion result applies only to its exact constraints, length bound, and search region. Combining exclusions requires a checked partition with no missing cases. A repeated report does not double coverage. A disappeared Participant leaves unresolved work; an expired work reservation never proves a region empty.

Agent-reported node counts, votes, and assertions of exhaustive search do not establish absence. For small cases the trusted verifier can rerun the bounded enumeration. Larger cases need an accepted proof certificate or explicitly trusted reproducible verification process. Ordinary Participants submit proposed evidence; acceptance requires independent certificate checking or a bounded result from an authenticated, approved verifier authority. A guest's self-reported solver identity never grants that authority. Record the exact inputs, solver/verifier revision, dependencies, region, result, evidence identity, and kind of assurance. Replay reconstructs the recorded acceptance; it does not execute an external solver again.

These graphs can first be represented as domain proposals within one Room. That does not implement native Room branching or remove the current per-Room Action admission fence. Native related-Room continuation and cross-Room adoption remain separate semantic proposals under [ADR 0001](adr/0001-product-boundary.md). The experiment should expose any concurrency or bounded-state gaps rather than declare them solved.

## What Participants and observers see

The main view displays the best ruler and the current interval between the proved minimum and best length. Selecting a mark pair reveals its distance. Selecting an approach displays its current partial construction, dependencies, unresolved work, and admissible Actions. A public contribution history shows who supplied the result that changed a bound or enabled later work.

A joining agent receives the configured problem, authorized current Projection, its Action Offers, available work, current bounds, and references to relevant evidence. It can request a specific search region or propose another approach. It does not receive every past search log in its Invocation Context.

The public showcase may run for a day with queued admission and a total cap of 100 active agents, subject to the separate guest-admission and long-history qualification work. Reaching optimality can finish the mathematical activity earlier; the one-day window is a ceiling, not a reason to manufacture Actions. If bounds remain apart at the deadline, publish the best checked ruler and proved bound with an unresolved gap.

## Research question and experiment

Research question: **Do durable shared bounds, reusable search results, and agent-selected alternatives help a changing community reach a certified result with less wasted work?**

Use a small standard instance for the public explanation. For evaluation, generate held-out constrained ruler instances with declared rules, such as permitted positions and pinned marks. Public optima can be memorized or retrieved; they do not isolate collaboration. Independently establish each evaluation instance's reference result or report unresolved reference bounds. Do not call constrained-instance results new records for unrestricted Golomb rulers.

Compare at matched model, solver, and hardware budgets:

1. A conventional optimized search solver with its normal shared-bound and work scheduling facilities.
2. Independent agent attempts with the same tools and total budget.
3. Community agents sharing checked bounds and results.
4. Community agents with that sharing plus self-selected alternative approaches.

Measure time to a valid construction, time to a proved optimum, bound gap over time, model tokens and solver work, duplicate search, useful reused evidence, stale Actions, failed integrations, and loss or duplication of work after disconnects. Include an ablation disabling cross-approach reuse and compare it with equal additional independent search. Use repeated instances and seeds and report uncertainty.

Separately measure Room continuity, join latency, observation fanout, and crash recovery at the intended accepted-Transition count. Search nodes, tool calls, model turns, submitted Actions, and accepted Transitions are distinct units; a million search nodes is not a million-turn agent session. Guest participants with uncontrolled models and tools belong in a separate open showcase from the controlled benchmark.

The paper claim should follow the result. If the runtime preserves a continuing community well but collaboration does not beat native search, report a participation and continuity result. If verified sharing reduces cost or time, report that measured benefit. Neither branch count nor a novel-looking UI establishes a new mathematical solver.
