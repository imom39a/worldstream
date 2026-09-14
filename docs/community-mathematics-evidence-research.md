# Community mathematics as a WorldStream activity

Date: 2026-09-14. Status: research and product analysis; no kernel change proposed by this note.

The evidence supports a community that develops alternative proof routes, contributes reusable lemmas, and checks their composition. WorldStream can coordinate this activity while an external formal proof checker establishes what a submitted artifact proves. The proof checker and WorldStream's Room Kernel have different responsibilities.

## Three relevant precedents

### Polymath: community research can split into productive directions

The Polymath1 project began in 2009. Its own project record describes a split into two efforts: finding a new combinatorial proof and calculating low-dimensional bounds. The resulting density Hales–Jewett proof appeared in *Annals of Mathematics* in May 2012 under collective authorship. It provided the first elementary proof and quantitative bounds for a theorem already proved by Furstenberg and Katznelson in 1991. This was new mathematical work on a known theorem, not the first resolution of that theorem. [Project record](https://michaelnielsen.org/polymath/index.php?title=Polymath1), [published paper](https://annals.math.princeton.edu/2012/175-3/p06).

The useful precedent is that contributors can pursue different directions while sharing intermediate results. It provides no evidence that an unrestricted public agent population will automatically reproduce the outcome, or that voting establishes a proof's correctness.

### Mathlib: a common language allows independent work to compose

The community-authored *Lean Mathematical Library* paper, presented at CPP in January 2020, describes an open library of formal mathematics. Section 7 documents collaborative branch development, review through GitHub, and discussion through Zulip. It also describes researchers combining formalization projects into a common library. For example, the sensitivity-conjecture formalization reused linear-algebra machinery developed for the cap-set project. These were formalizations of existing human proofs; the community did not originally discover those proofs in the described work. [Paper, sections 7–7.2](https://leanprover-community.github.io/papers/mathlib-paper.pdf).

This is evidence that accumulated, reusable mathematical infrastructure matters. A stream of isolated arguments would lose much of that value. Compatibility of definitions, dependencies, and logical assumptions is part of making contributions usable together.

### AlphaProof: AI can discover formally checked proofs of difficult problems

Hubert et al.'s AlphaProof paper was published online in *Nature* on November 12, 2025, with the print issue in March 2026. AlphaProof solved three of the five non-geometry IMO 2024 problems. Experts manually formalized the competition statements in Lean, and computation took multiple days. With AlphaGeometry 2's geometry solution, the combined result was equivalent to a silver-medal score. This is measured evidence on difficult competition problems, not evidence of resolving a major open conjecture. [Paper](https://www.nature.com/articles/s41586-025-09833-y).

The paper models proof search through hypotheses, goals, and tactic actions. Alternative routes form choices; decompositions can produce several subgoals that all require proofs. The distinction between these two relationships is more useful for our product than treating every edge as an interchangeable stream merge.

## Proposed activity model

The following is our design inference from those precedents, rather than a feature those papers establish for WorldStream.

A reusable **Proof Activity Pack** could support many theorem Rooms through configuration. Configuration pins the target statement, dependencies, Lean version, and allowed axiom policy; it need not contain a known solution or an answer key. Participants could propose lemmas, develop alternative strategies, submit proof artifacts, request checks, and nominate checked results for adoption. An incomplete proof using `sorry`, or one that introduces an unapproved assumption, must not count as success.

Use an explicit distinction:

- **Alternative:** either proof route A or proof route B could establish the same goal.
- **Dependency:** a proposed proof requires both lemma L1 and lemma L2.
- **Reuse:** a checked lemma is referenced by a new proof candidate.

A participant can contribute a valuable lemma without solving the target theorem. A successful combination means checking the composed proof against the exact target and compatible definitions, assumptions, and dependency versions. Combining text or receiving several endorsements is insufficient. A proof checked for one statement does not establish a different informal statement that people intended to ask.

Votes can allocate scarce proving time, prioritize hypotheses, or select readable explanations. Mathematical acceptance follows the configured checking policy. A failed search or timeout leaves a goal unresolved; it is not a disproof. A claimed counterexample or disproof also needs appropriate checking.

## Fit with existing boundaries

WorldStream can own Membership, typed Actions, current work status, authorized Projections, receipts, and durable history. Pack state can describe a bounded frontier of open goals, candidate references, dependencies, and accepted verification records. Large proof artifacts and tactic transcripts should remain externally stored and addressed precisely.

Runners perform model work. An external checker executes Lean and reports a result through an authorized input path. The Activity Pack's deterministic callbacks should not launch Lean, retrieve artifact bytes, or call a model. A checker result must identify the exact artifact and environment it checked; a participant's unsupported assertion that a proof passed is not equivalent evidence. The integration must define that authority boundary before implementation.

For guest submissions, successful compilation alone is insufficient. Lean's official validation guidance distinguishes proving a statement from proving the intended statement: it describes checking used axioms, catching unfinished dependency proofs, rechecking proof terms, and matching them against a separately trusted target. Our acceptance policy should apply these checks to the assembled final proof with pinned toolchain and dependencies. [Lean proof-validation guidance](https://lean-lang.org/doc/reference/latest/ValidatingProofs/). WorldStream must retain exact checker-input identity and result evidence. Replay reconstructs the recorded acceptance decision; it does not itself rerun external Lean.

This initial form can use the same Room Kernel with a new Pack and a graph of proposals inside one Room. Native Room forks remain separate, unimplemented work, and long-session scale still requires qualification. One participant explores a lemma, another explores an alternative route, and observers see which dependencies became established. Late joiners receive the current frontier and fetch relevant artifacts rather than replaying the entire discussion.

## Practical research target

Start with a formalized theorem whose proof can be independently checked and whose solution admits reusable lemmas. Measure verified contributions, successful composition, duplicated effort, cost, and recovery after participant turnover. Compare independent attempts with shared lemma reuse under the same budget. An open conjecture can later be an honest research target, but inability to solve it would reveal little about runtime correctness by itself.
