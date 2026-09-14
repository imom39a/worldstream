# Candidate: the shortest ruler with unique distances

Research checked 2026-09-14. Non-normative candidate assessment; no build or kernel change.

**Recommendation:** shortlist Golomb rulers as the strongest engineering and exact-verification candidate. Prefer circle packing or no-three-in-line if immediate visual comprehension and constructive combination matter most. Golomb rulers convincingly demonstrate distributed branching and accumulated proof, but should not be sold as agents merging pieces of one solution.

## The game

Place `n` integer marks on a ruler, beginning at zero. Every pair of marks must measure a different distance. Make the ruler as short as possible. A five-mark example is `[0,1,4,9,11]`: its ten pairwise distances are all different. Missing distances are allowed. The official project supplies this example and explanation. [distributed.net introduction](https://www.distributed.net/OGR)

A canvas shows marks above the ruler and occupied distances below it. Adding a mark highlights newly measured distances; a collision highlights the two conflicting pairs. Each branch shows its current partial ruler and remaining search region. Observers can distinguish finding a shorter ruler from proving that no shorter ruler exists.

Small established optimum lengths are 8 marks→34, 9→44, 10→55, 11→72, and 12→85. These are useful calibration targets, not new discoveries. The ASP competition publishes these values and benchmark instances. [Primary benchmark specification](https://dtai.cs.kuleuven.be/events/ASP-competition/Benchmarks/GolombRuler.shtml)

## Actual community evidence

distributed.net announced OGR-28 completion on November 23, 2022: approximately 8.5 years, over 65,000 volunteers in over 80 countries, and 524,091,443 search stubs, each completed independently at least twice with matching node counts. Exhaustive search confirmed the previously known optimum length 585; it did not discover a shorter ruler. This demonstrates long-lived volunteer partitioning and verification, not benefits from LLM reasoning. The announcement said there were no immediate plans for OGR-29. [Project's completion report](https://blogs.distributed.net/2022/11/23/03/28/bovine/)

The master assigns partial-ruler prefixes; clients search their descendants. Very small regions can be bundled to avoid excessive communication and tracking overhead. Work sizes vary dramatically under pruning, so naive percentage-complete estimates are misleading; clients can checkpoint interrupted work. These are directly relevant precedents for WorldStream admission, resumable branches, and honest progress metrics. [Search decomposition](https://faq.distributed.net/cache/281.html), [bundled stubs](https://faq.distributed.net/cache/298.html), [variable work and checkpoints](https://faq.distributed.net/cache/76.html)

## Meaningful work and verification

Useful contributions include extending a valid prefix, discovering a shorter complete ruler, splitting a large unexplored region, verifying another agent's result, or exhausting a bounded region. A new best length tightens every branch's upper bound. Agents choose which region or heuristic deserves their next bounded computation; voting may allocate compute, but cannot certify mathematical claims.

For a submitted ruler, the verifier checks increasing nonnegative integer marks, the requested count, and uniqueness of all `n(n−1)/2` positive differences. This takes quadratic pair enumeration with a set and exact integers. A collision is a compact, independently checkable rejection witness.

**Optimality requires more evidence.** A valid 12-mark ruler of length 85 establishes an upper bound only. Proving no valid ruler fits within length 84 establishes optimality. A claim that a branch was exhausted must be independently checked by bounded deterministic re-execution or an accepted proof certificate. Matching node counts alone is operational replication evidence, not a formal proof against shared algorithm bugs.

Branches compose through **verified coverage of a complete, disjoint partition**, plus shared bounds. They generally cannot combine partial mark sets: previously separate pairs can create duplicate cross-distances. This is a good honest conflict example, but a weaker constructive-merge showcase than its search-tree appearance suggests. Exhaustion can be valuable progress even when the displayed ruler gets no shorter.

## Proposed experiment

Use 10–12 marks to calibrate, then select difficulty empirically; do not promise a one-day duration from the mark count. Standard small solutions are public and may be memorized. Use held-out constrained instances with published forbidden positions, privately established optima, and the same exact rules for all participants. Report these as constrained ruler instances.

Compare an optimized native branch-and-bound solver, independent agents, a central shared queue, and agents choosing branches with shared bounds. Equalize tool CPU, model tokens, and validation budgets. Measure time to best solution, time to verified optimality, distinct regions closed, duplicate work, useful bound propagation, checkpoint recovery, and total cost. If agents add no improvement over the queue, the result may still support a runtime reliability claim; it does not support an intelligence claim. Never create one kernel Transition per internal search node merely to reach 100,000 turns.

## Comparison judgment

| Alternative | Relative tradeoff |
| --- | --- |
| Circle packing | More immediately visual; continuous optimization and numerical tolerances make exact validation and optimality harder. |
| Graph coloring | Straightforward edge checks and real decomposition possibilities; harder to explain why a particular graph is interesting. |
| No-three-in-line | Stronger board-game feel and exact integer checks; a strong rival when constructive visual progress is the priority. |
| Golomb ruler | Best documented volunteer-search precedent here; very simple exact certificate for solutions; weaker merge of constructive pieces. |

Choose Golomb rulers for **a community proving something together**. Choose another candidate if the flagship must visibly combine independently constructed solutions into a new solution.
