# Evidence for agents exploring alternative futures

Date: 2026-09-13. Status: research, not an accepted domain or kernel change.

Question: can agents preserve promising alternatives, choose which to pursue, and reuse or combine results without forcing everyone to agree on every next step?

The literature supports preserving and evaluating alternatives. It does **not** yet establish that arbitrary live environments can be forked and merged safely, or that a 100-agent, 100,000-turn deployment benefits from doing so. Those are distinct product and systems hypotheses. WorldStream could make this style of work observable, durable, and accessible to independent participants; branching reasoning itself already has substantial prior art.

This note uses eight main primary sources, with Go-Explore as a closely related foundation and MAKER as the existing Hanoi comparison. Results are authors' reported measurements, not reproductions. Versioned links preserve the exact experimental descriptions reviewed. Dates below distinguish initial preprints from later venues where relevant.

## Evidence map

| Work | What branches | What selects or combines results | What WorldStream can learn |
| --- | --- | --- | --- |
| Self-consistency | Independently sampled reasoning paths | Final-answer aggregation | Independent work is a meaningful baseline; discussion is optional |
| Tree of Thoughts | Partial reasoning solutions | Heuristic evaluation, search, backtracking | Preserve a bounded frontier of alternatives |
| Graph of Thoughts | Information dependencies | Task-defined aggregation and refinement | Combining information creates a new candidate that needs evaluation |
| LATS | Candidate action trajectories | MCTS with environment feedback | Exploration needs a resettable or isolated environment |
| Darwin Gödel Machine | Implementations of coding agents | Archive sampling and benchmark evaluation | Useful descendants can come from temporarily worse ancestors |
| AlphaEvolve | Candidate programs | Automated evaluators and population selection | Objective evidence makes selection more useful than popularity |
| Debate or Vote | Initially different answers | Voting and debate protocols | Include independent voting as a cost-controlled baseline |
| Scaling Agent Systems | Coordination architectures | Benchmark-specific task performance | More agents can hurt sequential tasks |

## 1. Self-consistency: keep independent alternatives before aggregating

Wang et al., initially **2022-03-21**, ICLR 2023. The method samples several reasoning paths and aggregates their final answers. It reports a 17.9 percentage-point gain on GSM8K over the chain-of-thought baseline. It does not retain an interactive environment per path, nor does it merge intermediate states. This is evidence for a simple independent-sampling control in our experiments. It is not evidence that prolonged discussion, explicit participant identities, or a branch runtime caused the gain. [Paper](https://arxiv.org/abs/2203.11171), [ICLR publication](https://openreview.net/pdf?id=1PL1NIMMrw).

**Implication:** compare any collaborative graph with independent attempts plus final verification at the same model budget. Otherwise extra sampling can be mistaken for a benefit from coordination.

## 2. Tree of Thoughts: defer commitment and revisit alternatives

Yao et al., initially **2023-05-17**; reviewed v2 dated 2023-12-03. ToT evaluates partial solutions and explores them with breadth-first or depth-first search, including pruning and backtracking. On the paper's 100 selected Game of 24 problems, GPT-4 reaches 74% success with breadth five, compared with 4% for the chain-of-thought baseline and 9% for 100-sample self-consistency. Its Game of 24 search has three intermediate steps; the work is not a long-session durability study. [Paper, sections 3–4 and Table 2](https://arxiv.org/html/2305.10601v2), [author implementation](https://github.com/princeton-nlp/tree-of-thought-llm).

**Implication:** agents can propose alternatives while a scheduler preserves only a budgeted active frontier. A branch may be promising without winning a global vote immediately. The runtime should expose evidence and lifecycle operations; it need not prescribe a search algorithm.

## 3. Graph of Thoughts: combining contributions requires a transformation

Besta et al., initially **2023-08-18**, AAAI 2024. GoT represents generated information and its dependencies as a graph. It supports generation, aggregation, scoring, and refinement. Aggregation creates a new thought derived from several prior thoughts. The paper reports a sorting-quality improvement of 62% over ToT alongside cost reduction exceeding 31% in its evaluated setting. These are task-specific results, not a generic merge guarantee. [Paper, sections 3–5](https://arxiv.org/html/2308.09687v4), [AAAI publication](https://ojs.aaai.org/index.php/AAAI/article/download/29720/31236), [author implementation](https://github.com/spcl/graph-of-thoughts).

**Implication:** separate provenance from validity. Recording that candidate C used A and B does not establish that C is correct. An Activity Pack should define what combination means, with C evaluated as new work. There is no universal semantic merge algorithm here for Room states.

## 4. LATS: acting branches need an environment that can be revisited

Zhou et al., initially **2023-10-06**, ICML 2024; reviewed v3. LATS combines Monte Carlo Tree Search, language-model proposals, value estimates, self-reflection, and environment feedback. Its cycle selects, expands, evaluates, simulates, and backpropagates value. The GPT-4 HumanEval result is 92.7% pass@1; that method internally samples five candidates during expansion over eight iterations, so pass@1 does not mean one model call. The paper explicitly recognizes the requirement to undo or revisit environment states. Text-only tasks make returning easy; real actions do not automatically share that property. [Paper, sections 3–5 and Table 4](https://arxiv.org/html/2310.04406v3), [author implementation](https://github.com/lapisrocks/LanguageAgentTreeSearch).

**Implication:** a durable counterfactual Room could support search that currently needs an application-specific simulator. External effects still need isolation or a proposal boundary. Copying a conversation cannot undo a payment, outgoing message, or resource allocation.

## 5. Darwin Gödel Machine: a weaker branch can lead to a better descendant

Zhang et al., initially **2025-05-29**; reviewed v1. DGM maintains an archive of coding-agent implementations and samples earlier variants for further modification. The winning lineage includes temporary performance declines. This is unusually direct evidence against retaining only the latest or currently highest-scoring variant. The experiment ran 80 iterations, with two parallel iterations for SWE-bench and four for Polyglot. It reports improvement from 20% to 50% on its SWE-bench Verified evaluation subsets, and from 14.2% to 30.7% on full Polyglot. The SWE-bench run took roughly two weeks and an estimated $22,000. [Paper, sections 4–5 and Appendix B](https://arxiv.org/html/2505.22954v1), [author implementation](https://github.com/jennyzzt/dgm).

**Implication:** preserve selected dissenting lineages and their evidence rather than requiring immediate majority endorsement. This supports the user's proposed experience. The objects evolving are agent implementations around frozen foundation models, not mutually editable live Room states; these experiments do not establish a 100,000-turn runtime. In WorldStream, an agent must not silently rewrite a Room's pinned Activity Pack Revision.

## 6. AlphaEvolve: use measured outcomes to allocate further exploration

Google DeepMind, **2025-05-14** announcement. AlphaEvolve produces candidate programs, runs automated evaluators, stores candidates and scores in a program database, and chooses candidates for further evolution. Google reports rediscovering the best known solutions in roughly 75% of more than 50 mathematical problems and improving best known solutions in 20%. These are first-party results for domains with executable evaluators; neither the model nor a crowd vote alone certifies correctness. [First-party system description and linked white paper](https://deepmind.google/blog/alphaevolve-a-gemini-powered-coding-agent-for-designing-advanced-algorithms/), [published mathematical problem artifacts](https://google-deepmind.github.io/alphaevolve_repository_of_problems/).

**Implication:** let agents nominate branches, while bounded evaluation work supplies comparable evidence. Store the evaluator identity, exact candidate identity, and basis with a score. A stale score is not automatic authority to promote into a changed target.

## 7. Debate or Vote: agreement does not establish added value

Choi, Zhu, and Li, initially **2025-08-24**, NeurIPS 2025. Across seven NLP benchmarks, most gains associated with multi-agent debate are attributable to majority voting. The main comparisons use five agents and two, three, or five debate rounds; majority voting is often comparable or better. Its martingale argument uses an idealized Dirichlet model and a condition that the neighbors' average belief equals the agent's current belief. It is not a theorem that every debate protocol is useless. [Paper, sections 3–4 and Appendix C](https://arxiv.org/html/2508.17536v1), [NeurIPS publication](https://proceedings.neurips.cc/paper_files/paper/2025/file/934252acd87f254d5d4672fbde283bd2-Paper-Conference.pdf), [author implementation](https://github.com/deeplearning-wisc/debate-or-vote).

**Implication:** do not require everyone to converge before any productive work continues. Let evidence affect selection and retain an independent-vote baseline. Voting can decide resource allocation or acceptance policy; correctness should rely on the relevant verifier when one exists.

## 8. Scaling agent systems: task structure changes whether collaboration helps

Kim et al., initially **2025-12-09**; reviewed v1. A controlled study covers 180 configurations, five architectures, three model families, and four agentic benchmarks with standardized tools and token budgets. All tested multi-agent variants degrade sequential planning performance by 39–70%, while some parallelizable tasks benefit substantially. The paper's limitations describe preliminary scaling only up to nine agents. Its numerical thresholds and regression coefficients are empirical findings for those configurations, not universal laws or a prediction for 100 WorldStream participants. [Paper, abstract, sections 4–5](https://arxiv.org/html/2512.08296v1), [author repository](https://github.com/ybkim95/agent-scaling/).

**Implication:** active branch count should depend on useful independence, evaluation capacity, and available budget. Reducing conflicts by isolating work is a systems hypothesis worth measuring; it does not by itself prove higher solution quality.

## Related foundation: Go-Explore

Ecoffet et al., **2021-02-24**, Nature. Go-Explore preserves promising environment states, returns to them, and then explores further. The original deterministic exploration approach is followed by robustification; another variant uses a goal-conditioned policy. This is a strong conceptual match for remembering useful checkpoints and revisiting them. Its experiments concern simulated reinforcement-learning environments, not arbitrary production state merging. [Paper](https://www.nature.com/articles/s41586-020-03157-9), [author implementation](https://github.com/uber-research/go-explore).

## Why MAKER is a different experiment

The earlier Hanoi paper, Meyerson et al.'s **2025-11-12** MAKER work, reports a correct sequence of 1,048,575 moves for 20 disks. It decomposes the task into microsteps and uses error correction with repeated proposals and first-to-ahead voting. Its central execution retains one accepted trajectory. It does not establish that several persistent future boards need to evolve and merge. [Paper](https://arxiv.org/html/2511.09030v1), [earlier local review](long-horizon-agent-papers-research.md).

For WorldStream, separate the claims being tested:

- **Durability:** can a Room accept and recover a large sequence of verified contributions with bounded agent context?
- **Exploration:** does preserving competing continuations improve success or cost compared with immediate consensus?
- **Combination:** can independently produced contributions be safely reused together?

Classic three-peg Hanoi with a fixed initial and target tower has a known optimal policy. Following that policy at scale is useful for durability, but gives little room to demonstrate the value of exploratory branching. A puzzle with deceptive partial progress, or a constrained planning problem with several viable strategies, is a more revealing exploration experiment. Combining two different complete Hanoi boards would not be an appropriate generic merge demonstration.

## Proposed research direction

This is a hypothesis for product research, not an accepted extension to WorldStream's domain model:

> Independent participants can preserve, inspect, and resume alternative continuations from a shared checkpoint, then contribute verified results to a selected target without erasing their provenance.

It should support several outcomes for a branch: continue independently, archive for later use, offer a result to another branch, or become the selected continuation. Every branch need not merge. Agents can decide where to work from bounded branch descriptions and exact authorized views; they do not need every other branch's entire history.

The first experiment should compare, at equal model-call or token budget:

1. One agent with sequential verified actions.
2. Independent attempts with final verification.
3. Immediate vote or consensus after each step.
4. Bounded alternative branches with periodic evaluation and selective reuse.

Measure solved instances, time and tokens per verified solution, duplicate work, invalid or stale actions, evaluator disagreement, branch survival, context bytes, and time to resume. Add a case where temporary loss of score is necessary to reach the best solution; otherwise aggressive pruning may appear adequate simply because the benchmark lacks that difficulty. Report model capability and policy along with the runtime measurements.

Use votes initially for deciding which experiments deserve resources, not as universal proof that their result can be combined. For reuse, distinguish copying a verified artifact, replaying a legal action sequence against a compatible target, and constructing a new result from several artifacts. Each has different validity rules.

## Relationship to current WorldStream boundaries

[CONTEXT.md](../CONTEXT.md) defines one authoritative shared situation per Room. [ADR 0001](adr/0001-product-boundary.md) currently excludes timeline forks, branch promotion, and cross-room data exchange. Research can inform revisiting those boundaries; these papers do not override them. Native durable branches require an explicit lineage and authority design. A bounded graph of candidate proposals inside one Activity Pack can test the product hypothesis without claiming native Room forks.

The most defensible potential differentiation is therefore the combination of independently hosted participants, durable lineage, scoped observation, measured branch selection, and domain-validated result adoption. The literature supplies the search rationale. We still need evidence that developers need this particular runtime contract and that it improves their real workloads.
