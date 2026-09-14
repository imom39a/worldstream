# Discrete math community candidate: no-three-in-line

2026-09-14 — research proposal only. Stronger of the two candidates considered: **no-three-in-line on a grid**, ahead of small Ramsey edge-coloring. This is a recommendation for an approachable experiment, not evidence that it is the best overall WorldStream showcase.

## Rule and evidence

Place as many points as possible on an integer grid without any three on one straight line, including non-diagonal slopes. On an ordinary n×n grid, two points per row gives the upper bound 2n. Published constructions attain 16 on 8×8, 24 on 12×12, and 32 on 16×16; the author's enumeration database provides the corresponding solution classes. A checked construction meeting 2n therefore proves optimality for that instance. [Flammenkamp's research and checker](https://wwwhomes.uni-bielefeld.de/achim/no3in/readme.html), [enumeration table, updated 2026-09-11](https://wwwhomes.uni-bielefeld.de/achim/no3in/table.html)

This remains an active construction problem at larger sizes. Prellberg's September 2025 report supplies new configurations and verification code; Riley's public CUDA implementation enumerates solutions. These are concrete computational precedents, and also strong baselines against which any agent experiment must be compared. Their existence does not imply that language agents improve search. [Prellberg report](https://wwwhomes.uni-bielefeld.de/achim/no3in/Prellberg_Sep_2025.pdf), [Riley's implementation](https://github.com/mvr/no-three-in-line)

For Ramsey, the corresponding approachable instruction is “color every edge red or blue without making a monochromatic clique.” Greenwood and Gleason proved R(3,3)=6 and R(4,4)=18, including a 17-vertex construction for the latter. Five vertices is too small for sustained collaboration; seventeen already has 136 edges. The grid offers a clearer spectator view and simpler incremental explanation. This UX comparison is our judgment. [Original 1955 paper](https://doi.org/10.4153/CJM-1955-001-4)

## Proposed experience

Start with a 12×12 or 16×16 grid containing blocked cells and a few mandatory points. These are **completion variants**, distinct from claims about the classical open problem. Calibrate difficulty before choosing the size or promising a day-long challenge. A day-long Room may require a fixed suite of instances rather than one puzzle.

Each branch is a shared partial construction and explicit assumptions. Agents can join an existing attempt, propose an atomic remove/add patch, test a different symmetry or placement strategy in a fork, request a repair of a congested region, or import a useful partial pattern. Spectators see colored contributor points, the selected branch, ghosted proposed patches, and offending lines on rejected imports. Small branch previews show placed points, target, unresolved constraints, and contributors.

The Pack maintains authoritative selected points, exclusions, mandatory points, immutable patch provenance, outstanding repair requests, and checked results. External agents choose strategies and run calculations. Late joiners receive the current construction and open requests rather than all prior discussion. Votes can allocate effort or endorse a precise candidate revision; they cannot make an invalid geometry valid.

## What genuinely combines

Compatible placements and repair patches can accumulate across contributors. Merge is substantive: from base {(0,0)}, one branch adds (1,1), another adds (2,2). Each branch is legal; their union is not. The merge must return that exact three-point witness and preserve both alternatives for repair. Disjoint coordinates alone do not establish independence.

An agent can retain one branch's useful rows and replace the conflicting part with another contributor's repair. Conditional conclusions stay attached to their assumptions. A generalized “these choices cannot reach the target” claim needs a checked proof before other branches use it for pruning; an unsuccessful search is not such a proof. Avoid making arbitrary natural-language lemmas authoritative.

Validation uses integer cross products for every triple, coordinate bounds, uniqueness, and instance restrictions. Reaching 2n proves optimality even with blockers and mandatory points because the row bound still applies. A smaller valid arrangement proves only a lower bound. Global optimality below 2n requires separate exhaustive or proof-producing analysis, not a completion badge from the geometry checker.

## Research controls and limitations

Generate blocked/pinned instances after freezing prompts and policies; publish generator commitments and later release seeds. Verify each target offline, retain the witnesses separately, and filter geometrically equivalent instances. Mere rotations or relabeling of known solutions do not address memorization. These controls reduce answer reuse; they do not prove uncontaminated reasoning.

Compare equal-budget shared-state, independent best-of-N, and collaborative-branch conditions, plus a conventional solver. Measure time to verified target, cost, rejected imports, attributable reuse of another agent's partial work, and performance when imports are disabled. If cooperation does not outperform independent attempts, describe the system as a distributed search demonstration. The core experiment must involve durable partial contributions and repair, not just final point-list submissions or fixed brute-force partitions.

An existing public agent challenge, Zerothesis, already accepts no-three-in-line solvers with exact evaluation and an experiment journal. The puzzle itself therefore supplies no product differentiation. [Published challenge](https://zerothesis.com/challenges/no-three-in-line?tab=brief)
