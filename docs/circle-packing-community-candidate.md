# Candidate: a community circle-packing laboratory

Date: 2026-09-14. Status: one candidate for product research; no implementation.

## The precise activity

**Place 26 circles inside a unit square, allow different radii, and maximize the sum of their radii.** Every radius is positive; circles may touch but their interiors must not overlap. This matches the AlphaEvolve benchmark and has an immediately readable canvas. The score measures total radius, not occupied area.

For circle centers `(x_i, y_i)` and radii `r_i`, require:

```text
r_i <= x_i <= 1 - r_i
r_i <= y_i <= 1 - r_i
(x_i - x_j)^2 + (y_i - y_j)^2 >= (r_i + r_j)^2
score = sum(r_i), for i = 1..26
```

Do not confuse this with 26 **equal** circles maximizing one common radius. Packomania currently lists approximately `0.096362339010` for that separate problem. The variable-radius table lists approximately `2.635983084919` for 26 circles and was updated September 13, 2026. These are reference constructions, not an answer key or a claim of global optimality. Pin downloaded coordinates and retrieval date before using them in an experiment. [Equal-circle table](https://packomania.com/csq/csq.html), [variable-radius table](https://packomania.com/csqv/csqv.html).

## What the evidence establishes

AlphaEvolve's original report describes improving the 26-circle sum from about `2.634` to `2.635`. Its later mathematical report explains that the initially published coordinates were truncated, that others refined their precision, and that such small improvements can largely reflect more computation. Full-precision constructions are now published. Therefore, surpassing the rounded `2.635` is not evidence of a new result or effective agent collaboration. [Original report, B.12](https://storage.googleapis.com/deepmind-media/DeepMind.com/Blog/alphaevolve-a-gemini-powered-coding-agent-for-designing-advanced-algorithms/AlphaEvolve.pdf), [mathematical report, Problem 6.36](https://arxiv.org/html/2511.02864v1), [author artifacts](https://google-deepmind.github.io/alphaevolve_repository_of_problems/).

A January 2026 study used unmodified SCIP and FICO Xpress with direct nonlinear formulations to match or improve several AlphaEvolve geometry results. For the related 32-circle square problem it reports `2.93957`, compared with `2.93794`. This is strong evidence that a numerical optimizer is an essential baseline and useful agent tool. It does not prove that this 26-circle candidate needs an LLM, let alone a community. [Berthold et al., sections 3 and 5](https://arxiv.org/html/2601.05943v2).

## What participants actually contribute

The product should expose a shared investigation, with progress beyond a ranked list of final scores:

- Propose a new arrangement family: boundary rows, a central cluster, approximate symmetry, or another contact pattern.
- Optimize an existing candidate using a different solver, formulation, perturbation, or restart strategy.
- Identify which contacts constrain improvement and propose a local rearrangement.
- Convert an attractive floating-point result into a rigorously feasible certificate.
- Reproduce another participant's construction and contribute a reusable search program or diagnostic.

These are proposed roles, not demonstrated performance advantages. Useful branches may initially score worse because they explore a different geometric basin. Retain a bounded set of distinct arrangements, their origin, method, computation spent, and validation evidence. Agents can choose one to extend without needing unanimous agreement.

Combining work means applying a shared heuristic or trying a proposed local modification and then rechecking the resulting whole arrangement. Copying clusters from two packings can introduce overlaps across their boundary. The combined candidate receives its own identity and check; provenance alone does not establish feasibility.

## Exact validation is simpler than proof of optimality

For the first version, accept coordinates and radii on a documented rational grid. With common denominator `Q`, submit integers `X_i, Y_i, R_i`. A checker uses exact integer arithmetic for the boundary inequalities and all **325** pairwise squared-distance inequalities. The score is the exact rational `sum(R_i)/Q`. This is our proposed certificate format and follows directly from the constraints above.

Ordinary floating-point solver output remains a candidate until certified. Rounding coordinates may create overlaps; reducing radii and checking again can yield a slightly weaker but rigorous certificate. Use arbitrary-precision integers or a proved overflow bound.

A passing certificate proves that **this arrangement exists**, giving a lower bound on the continuous optimum. It does not prove no better arrangement exists. A global-optimality claim needs a separately checked upper-bound argument covering every feasible configuration. The rational-grid optimum would itself be a different claim from the continuous optimum. Label results accordingly.

## WorldStream fit and decision criterion

A Packing Activity Pack can manage candidates, derivation links, review requests, votes for allocating effort, and checked acceptance in one Room. Numerical search runs externally. The Pack can either check a strictly bounded integer-coordinate Action deterministically, or accept exact recorded results from an authorized checker; neither path runs a solver inside the Pack. Late participants need the current frontier and relevant candidate artifacts, not the entire history.

Native Room forks are unnecessary for this initial candidate graph. It tests whether agents selecting alternative work and reusing contributions helps before introducing native branch semantics.

Run an equal-budget comparison against multistart numerical optimization and independent agent attempts. Measure certified improvement, time, cost, distinct useful arrangements, duplicated work, and successful reuse. Include already published constructions transparently. The credible success claim is improved collaborative search and durable contribution reuse. If a standard optimizer achieves the same result faster and sharing adds no measurable benefit, retain this as a clear validation demonstration rather than the flagship differentiator.
