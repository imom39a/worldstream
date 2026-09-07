# Evaluation methods for community agent scenarios

> **Status:** Research and proposed product direction. Not an accepted ADR or an implementation claim.
> **Research date:** 2026-09-03
> **Scope:** Human-created games, cooperative challenges, and social experiments involving configurable agents and, where permitted, human participants.
> **Primary starting points:** [TextArena, v2](https://arxiv.org/html/2504.11442v2) and [MindGames, v1](https://arxiv.org/html/2605.29512v1).
> **Boundary:** This note does not change the frozen WorldStream roadmap, rename the project, or authorize public hosting of contributed code. Product-level terms below are proposals, not additions to the canonical domain model.

## 1. Executive recommendation

Build a platform that helps people ask a precise question, run a scenario, and inspect the evidence. Do not make a universal agent leaderboard the foundation of every activity.

The proposed product promise is:

> Create a challenge, bring your agents, and measure what happens under clear rules.

Use five separate result cards:

1. **Outcome:** Did the participants achieve the goal? Who won, where winning applies?
2. **Observed behavior:** What did participants actually do in this scenario?
3. **Validity and reliability:** Were actions valid? Did runs complete? Does success repeat?
4. **Efficiency:** What time, tokens, tool calls, and other resources did the attempt use?
5. **Robustness:** Does performance hold with different partners, seats, and scenario instances?

These cards answer different questions. Do not hide them inside one weighted score. A creator can declare a primary metric for one experiment, but must publish its definition and keep the supporting results visible.

The research supports a practical first implementation: deterministic outcome checks, repeated controlled runs, explicit failure accounting, and per-game competitive ratings. LLM judging and advanced population-ranking methods are optional additions, not prerequisites.

## 2. What the primary sources establish

The source findings below are descriptive. The proposed application of each method is our engineering judgment; none of these papers validates this proposed platform.

### TextArena: the starting platform model

TextArena uses a shared text interaction interface and online TrueSkill ratings. Its paper also describes skill profiles formed from weighted game ratings, with presentation-normalized skill charts. It acknowledges that gameplay results include rule and format comprehension. [TextArena, sections 2 and 4; figures 1–2](https://arxiv.org/html/2504.11442v2)

**Adopt:** simple agent interfaces, extensible scenarios, game-specific outcomes, and uncertainty-aware competitive ratings.

**Do not assume:** tagging several games with “theory of mind” makes their weighted scores a validated measurement of that general ability. We should initially treat skill tags as discovery metadata. Any broad capability claim needs separate validation.

### MindGames: evaluate the measurement, not just the standings

MindGames combines a game suite with trajectory analysis and error attribution. It identifies cases where standings partly reflect opponents' failures. Its MG-Ref protocol specifies controlled reference opponents, scheduling, and multiple reported measures. [MindGames, sections 5 and appendix J](https://arxiv.org/html/2605.29512v1)

**Adopt:** report failures alongside outcomes and keep a fixed-reference evaluation distinct from the live ladder.

**Caution:** results on error-free games are a selected subset. They can help diagnosis, but are not automatically an unbiased estimate of strategic ability. Removing failed games can change the population being measured.

### TrueSkill: useful competition ratings, not a general evaluator

TrueSkill models uncertain skill and updates it from individual or team outcomes, including draws. Its team model assumes performance contributions combine additively. [Herbrich, Minka, and Graepel, 2006](https://proceedings.neurips.cc/paper/2006/file/f44ee263952e65b3610b8ba51229d1f9-Paper.pdf)

**Adopt:** one rating pool per declared scenario configuration and competition division. Show rating uncertainty and match counts.

**Limit:** inferred team-member skill does not establish causal contribution. A cooperative activity with no opposing teams does not need an invented winner just to use TrueSkill.

### AlphaRank: a warning against forcing every contest into a line

AlphaRank uses an evolutionary-game approach to rank strategies in multi-agent interactions, including asymmetric and nontransitive settings. Its analysis operates over strategy profiles, whose number can grow rapidly. [Omidshafiei et al., 2019](https://arxiv.org/abs/1903.01373)

**Adopt now:** preserve matchup matrices for two-player games and joint-profile payoff records for larger games. If A beats B, B beats C, and C beats A, show that relationship instead of suggesting a universally strongest agent. Do not decompose a multiplayer result into supposedly independent pairwise wins.

**Defer:** implementing AlphaRank until a scenario demonstrates that a scalar rating is materially misleading and enough matchup evidence exists.

### Statistical reliability: repeat trials and preserve the sampling structure

The rliable work studies uncertainty and aggregation in few-run reinforcement-learning evaluations, including stratified bootstrap intervals, robust aggregates, and selection bias. Its examples show that small-run estimates and even some bootstrap intervals can be unreliable. [Agarwal et al., 2021](https://arxiv.org/html/2108.13264v2)

**Adopt:** report uncertainty, predeclare aggregation, and separate tuning from confirmation.

**Limit:** its experiments are not a universal recipe for LLM tournaments. Our resampling unit must reflect our own dependencies; copying a library call is insufficient.

### Chatbot Arena: distinguish observations from a fitted ranking

Chatbot Arena develops preference-based evaluation using pairwise data, statistical ranking, and uncertainty analysis. Its methodology also addresses sampling and distinguishes observed comparisons from fitted scores. [Chiang et al., 2024](https://proceedings.mlr.press/v235/chiang24b.html)

**Adopt:** publish the underlying comparison evidence and avoid presenting uncertain rank differences as settled facts.

**Limit:** human preference over answers is a different measurement from winning a game or completing a shared task. We should not transfer its score interpretation unchanged.

### Melting Pot: cooperation must work with unfamiliar partners

Melting Pot evaluates agents against held-out background populations and varied social situations, rather than measuring only interaction with familiar policies. It supports examining effects on other participants as well as the focal population. [Leibo et al., 2021](https://proceedings.mlr.press/v139/leibo21a/leibo21a.pdf)

**Adopt:** test homogeneous teams, mixed teams, and held-out partners. Report joint outcomes and who benefits or loses.

**Limit:** transfer the evaluation design, not an assumption that success in its environments predicts success in our scenarios. No reinforcement-learning training infrastructure is required for this adaptation.

### SOTOPIA: useful social rubrics, with dimension-specific validity

SOTOPIA evaluates interactions involving private goals and partial observations using several social dimensions. Its validation shows that agreement between automated and human evaluation varies by dimension. [Zhou et al., SOTOPIA](https://arxiv.org/html/2310.11667v2)

**Adopt:** explicit goals, anchored rubrics, and separate scores for separate questions.

**Limit:** a judge that works for goal achievement is not thereby validated for secrecy, relationship quality, or fairness. Calibrate every dimension we intend to publish.

### Social simulation: convincing dialogue is not evidence of valid interaction

A comparison of omniscient script generation with independently acting agents finds important differences in apparent goal achievement. Fluent simulated dialogue can conceal the difficulty of acting with private information. [Zhou et al., The Misleading Success of Simulating Social Interactions With LLMs](https://arxiv.org/html/2403.05020v2)

**Adopt:** independent participant observations and explicit information boundaries.

**Limit:** experiments describe the tested agents under the specified simulator. They do not establish how human communities behave.

### LLM-as-judge: calibration is part of the feature

The MT-Bench judge study documents position and verbosity biases, reasoning limitations, and differences among judging setups. Its agreement results depend on the dataset and treatment of ties. [Zheng et al., 2023](https://arxiv.org/html/2306.05685v4)

**Adopt:** blinded identities, order-swapped comparisons where applicable, concrete rubrics, and held-out human calibration.

**Limit:** do not reuse a headline agreement percentage as our judge's accuracy. Multiple agreeing model judges can share the same bias.

### Tau-bench: outcome verification and repeated success

Tau-bench checks resulting state and required outputs, and defines pass^k for success across repeated trials of the same task. It notes that its reward can still miss a policy violation. [Yao et al., 2024, section 3](https://arxiv.org/html/2406.12045v1)

**Adopt:** check actual state, not an agent's claim of completion. Test required conduct separately when the final state cannot prove it.

**Limit:** pass^k is not pass@k. The former asks whether all k attempts succeed; the latter asks whether at least one does. Both require a declared repeated-trial protocol.

### Practical engineering guidance

Anthropic distinguishes the final outcome from the interaction transcript, discusses code-, model-, and human-based graders, and recommends inspecting trials rather than trusting scores alone. This is first-party engineering guidance, not a research validation of our design. [Demystifying evals for AI agents, 2026](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)

**Adopt:** retain auditable evidence and evaluate the complete agent setup, not only the model name.

## 3. Choose the evaluator from the question

The following table is a proposed product policy.

| Scenario family | Primary measurement | Supporting evidence | Avoid |
| --- | --- | --- | --- |
| Competitive game | Pack-defined result; per-game rating where appropriate | Win/draw/loss, joint-profile payoffs, seats, errors, uncertainty | One ranking across unrelated games |
| Cooperative challenge | Verified group success and constraint compliance | Raw group utility, unmet needs, individual utilities, partner variation | Treating every group success as equal individual contribution |
| Negotiation or mixed incentives | Individual and joint utility under the declared rules | Agreement rate, outside options, resource distribution, deadline failures | Calling agreement alone a good outcome |
| Social interaction | Narrow outcome checks plus validated rubric dimensions | Evidence-linked judgments, rater disagreement, visibility conditions | A universal trustworthiness or personality score |
| Controlled experiment | Difference between predefined conditions | Effect size, uncertainty, run allocation, exclusions, sensitivity checks | Inferring causation from a live leaderboard |

For example, a resource-allocation activity must state whether “fair” means equal amounts, equal coverage of need, or a guaranteed minimum. These are different goals. A high total payoff can coexist with one participant receiving nothing.

## 4. Three clearly labelled operating modes

### Practice and exploration

Users can change prompts, try configurations, and inspect individual runs. Results are useful feedback, not a confirmed comparison. Show costs and known failures even here.

### Controlled evaluation

Freeze the tested setups and the evaluation plan before collecting confirmation data. Balance relevant conditions, repeat trials, and publish uncertainty. The result applies to the declared population and scenarios.

### Reference comparison

Use a pinned reference implementation, configuration, observation format, parser, evaluator, and opponent set. Report deviations. New custom rules belong to a different profile, even if the game has the same familiar name.

A contribution approved for hosting is not thereby a validated benchmark. Publication, operational approval, and evaluation quality are distinct decisions.

## 5. What a controlled evaluation must freeze

An external evaluation specification should record the following. This is a design checklist, not a supported file format or a new Kernel API.

- **Question:** the comparison being made, primary metric, direction of improvement, and any practically meaningful effect threshold.
- **Scenario:** exact Activity Pack Revision, configuration, initial conditions, and observation/interface versions.
- **Agent entry:** model identifier, provider-reported version where available, decoding settings, strategy and skill versions, tools, and Runner/agent code version.
- **Assistance:** autonomous or human-assisted mode, intervention rules, and what is logged.
- **Memory:** reset between matches or retained learning; retained-memory competitions need their own sequential protocol.
- **Population:** exact partner and opponent pool, including reference versions and sampling weights.
- **Schedule:** scenario instances, seats, roles, speaking order, seeds, repetitions, and randomized execution order.
- **Limits:** action, token, tool, time, and monetary caps where enforceable; source and completeness of usage measurements.
- **Scoring:** evaluator version, metric definitions, thresholds, numerators, denominators, and invalid/aborted-run policy.
- **Analysis:** independent sampling unit, interval method, aggregation weights, stopping rule, and development/evaluation split.
- **Evidence access:** what participants, evaluators, reviewers, and public spectators may see.

A self-reported model name or tool budget is not verified execution. Label external submissions accordingly. A controlled execution path can attest to what it controls; it cannot promise that a mutable provider alias identifies unchanged internal weights. Never publish API keys or require disclosure of private strategy text merely to identify a version.

## 6. Experimental design and statistical uncertainty

### Compare complete agent configurations

To test whether a human's strategy helps, begin with the same model, tool allowance, and budget. Change one defined part of the setup. If several components change together, the result measures the combined setup, not the individual component.

Do not choose the best prompt on the evaluation set and then present that same result as confirmation. Freeze the selected entry and test it on fresh, held-out cases. A repeatedly inspected holdout eventually becomes development data.

### Balance and randomize

Use a fixed schedule that balances roles and opponent exposure. For an A/B comparison, pair configurations on the same scenario instances, partner configurations, and seat assignments where possible. Randomize their execution order to reduce systematic time effects.

Matching environment seeds does not force two LLM runs to make the same choices or encounter the same later random events after their actions diverge. Match initial and exogenous conditions where meaningful. Record concrete initial assignments and model settings; retain actual observations and actions. Do not describe a replay of recorded decisions as a reproduced model inference. Reset both focal-agent and partner state when the protocol requires independent episodes.

For human-involved experiments, declare the population and assistance conditions. Account for learning and repeated participation; do not pool every person into one supposed representative “human” baseline.

### Count the correct unit

A six-player match is one joint event, not six independent trials. Ten turns are not ten independent tests of the final outcome. Repeated matches that share a scenario instance or paired role-swap design may form a larger block.

For paired comparisons, calculate the difference within each matched block, then estimate uncertainty across independent blocks. If using a bootstrap, resample complete blocks and preserve their dependent results. Retain any predefined scenario/role strata and their target weights. With too few blocks, label uncertainty as unreliable rather than inventing precision.

### Do not declare a universal minimum match count

As a simple illustration, 20 wins in a fixed sample of 30 independent, identically distributed binary, no-draw matches is a 66.7% estimate, but its two-sided 95% Wilson interval is approximately **48.8%–80.8%**. Assume one fixed evaluation distribution, not selected wins or a stop-when-ahead rule. The interval is wide enough to matter. The calculation uses the [NIST Wilson interval method](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm).

Sample planning should depend on the required precision, expected variance, effect of interest, and dependencies. Use a small pilot to estimate cost and variability, then fix the confirmation plan. Do not keep collecting until an ordinary confidence interval happens to look favorable.

TrueSkill's posterior uncertainty is not the same quantity as a confidence interval for a win rate. Label them separately. Many simultaneous comparisons need appropriate multiplicity handling or an exploratory label; overlapping or non-overlapping marginal intervals are not a universal significance test.

### Keep the reference and live populations separate

A live ladder describes performance against its changing pool. A benchmark against frozen references answers a different question. Freeze the reference configurations, scenario weights, budgets, scorers, and inference procedures, not only their rating numbers. Direct outcomes against that panel are the primary comparable evidence; an anchored rating is a summary. Provider changes can still limit comparability. Do not silently mix the two modes or interpret changing live ratings as pure agent improvement.

Publish per-opponent results and coverage. A pool dominated by a weak, repeatedly farmed baseline should not produce a strong general claim. For cross-scenario reporting, retain a vector of results; only aggregate when weights and normalization have a declared interpretation.

## 7. Make failures visible

Record every planned and attempted run with explicit status. At minimum, distinguish:

- valid completion;
- game-invalid action and its Pack-defined consequence;
- agent deadline or declared budget exhaustion;
- provider or Runner failure;
- infrastructure interruption;
- evaluator failure or unavailable evidence;
- cancellation and any replacement attempt.

Define which statuses enter each metric before the run. Show planned, attempted, completed, aborted, and retried counts. Report end-to-end outcomes over assigned attempts as well as gameplay outcomes conditional on valid execution. Explain which statuses are agent failures and which invalidate a trial for infrastructure reasons. An evaluator failure is not a score of zero. An operational exclusion is not permission to delete its record.

Link retries to their original attempts and enforce a fixed retry policy. A retry must not become an opportunity to choose the best result. Preserve the paired-block design when handling failed or replaced trials, and disclose any incomplete blocks and resulting analysis changes.

Preserve literal model actions and any default or repair applied by an adapter. Otherwise a forgiving parser can appear to be a more capable agent. A repair that changes the permitted behavior belongs in the versioned interface or agent setup.

For benchmark-compatible text games, a typed `submit_move` envelope can carry raw text. A well-formed, authorized submission with an illegal game move can then become a recorded domain attempt, including a penalty if the Pack defines one. Authentication or stale-state rejection remains a different event. This is a proposed integration pattern, not a change to the accepted Kernel contract.

## 8. Judge policy for open-ended work

Use this proposed order of preference:

1. **Deterministic checks** for facts the environment can establish: allocations, delivered objects, legal moves, balances, deadlines, and defined goals.
2. **Human review** for criteria that need interpretation, especially while the rubric is new.
3. **Calibrated model judging** for scaling specific dimensions once validation is adequate for the intended claim.

Each judged dimension needs a question, anchored score descriptions, allowed evidence, and an “unscorable” option. Require evidence references, but do not assume a cited event makes the interpretation correct.

Before using a model judge in rankings:

- evaluate it on a held-out human-labelled sample, separately for each dimension, including actual conduct violations and non-violations;
- report false-positive and false-negative rates for categorical checks, not only aggregate agreement;
- report human disagreement, not just model-versus-consensus agreement;
- blind entrant identity where feasible;
- test response-order and verbosity sensitivity;
- include counterexamples where eloquent claims conflict with actual state;
- test evaluator-directed instructions embedded in participant messages;
- define an escalation path for disagreement or missing evidence.

Evaluator input is untrusted data. A model grader should not have tools, credentials, or network access that participant text could cause it to use. It must not grant itself broader evidence access.

An evaluator may need private goals or hidden state after a match. That does not authorize disclosure to participants or public spectators. Keep private evaluation records and public reports separate. Internal model reasoning is not required evidence; do not claim to observe hidden intent from a transcript.

## 9. Two concrete first evaluation packages

These are proposed examples, not reported experiment results.

### A. Three-player cooperation and defection

**Question:** Does bounded pre-decision communication improve collective payoff under a specified payoff table and agent population?

**Design:** Compare communication-enabled and communication-disabled configurations. Freeze model/strategy versions, decision rules, partner pool, and total budgets. Balance seats and reset agent-private memory between independent matches. Declare that communication consumes part of the common budget; this tests its benefit under that budget, not the effect of communication with free extra compute.

**Primary metric:** difference in raw collective payoff across matched scenario blocks.

**Supporting metrics:** each participant's payoff; explicit cooperation frequency; defaulted decisions; invalid outputs; token use; completion rate; partner-conditioned outcomes.

Count cooperation over declared decision opportunities toward opponents. Report parser-inserted cooperation separately from an agent's explicit choice. Keep raw utility separate from any terminal rank reward.

Use supplied scripted baselines to test the scorer. Their expected actions and payoffs should be independently calculable. They help validate the experiment, but do not by themselves establish performance against a diverse population.

**Permitted claim:** “Under these agents, rules, and budget, allowing communication changed mean group payoff by this amount, with this uncertainty.”

**Not permitted by this test:** “These agents are generally trustworthy,” or “communication improves cooperation in human society.”

A separate strategy experiment can hold communication rules fixed and compare a baseline entry with a human-guided entry. Do not change both communication access and guidance, then attribute the result to guidance alone.

### B. Cooperative resource allocation

**Question:** Can agents meet defined minimum service levels while using a limited shared supply?

**Design:** Specify district needs, available resources, participant information, legal transfers, and the time horizon. Include feasible and explicitly labelled infeasible instances. Compare a default policy against a human-guided policy with both familiar and held-out partners.

**Primary metric:** fraction of feasible instances in which every required service constraint is met within the declared limits.

**Supporting metrics:** unmet demand, total useful delivery, minimum district coverage, wasted resources, run completion, and cost. Publish infeasible-case behavior separately; do not blame an agent for an impossible success condition.

To study one agent's contribution, replace that policy with a fixed baseline and rerun matched blocks while holding partner policies and scenario conditions fixed and resetting all episodic memory. This estimates the replacement's effect under the chosen partner/role distribution. Replacing the whole team answers a different question. Neither is an intrinsic credit score. Conversation volume is not contribution.

For repeated binary success on the same case, a pass^k-style reliability measure can supplement success rate. Keep per-case repeated-trial counts and independence assumptions; do not raise an overall success rate across different tasks or teams to the kth power.

## 10. Low-friction creation without weak measurement

A template should supply an evaluation preset as well as rules. The creator should normally answer only:

1. What outcome matters?
2. Which roles, information, and resources are available?
3. What change or agent setup do you want to compare?
4. What limits can you afford?

The interface then shows an **evaluation card** in plain English: the primary metric, what counts as failure, the planned comparisons, evidence visibility, estimated run count, and claim limitations. Prompt assistance can draft supported settings; the person reviews them before execution.

For developer contributions, require metric definitions, known-answer fixtures, failure cases, privacy tests, and documentation of evaluator assumptions. Review new graders as code with their own trust boundary. A contribution review must not silently grant a self-hosted installation permission to execute it.

Do not require a creator to invent statistics. Ship a small set of reviewed presets. Do not allow a generated description to silently redefine scoring after results are visible.

## 11. Proposed implementation boundary

The pipeline is: frozen evaluation specification → scheduled Rooms → scoped run evidence → versioned evaluators → reproducible reports.

- **Activity Pack:** authoritative rules, domain scoring, visibility, and Outcome.
- **WorldStream:** accepted state transitions, scoped participation, recovery, and canonical Replay.
- **External Runner:** agent execution and appropriately protected model/tool telemetry.
- **Platform evaluation service:** repeated-run scheduling, population control, derived metrics, judge execution, and statistical analysis.
- **Activity Client/platform UI:** setup, observation, results, and evidence inspection.

Derived analysis must not rewrite a Room's Outcome. A changed analytics method produces a new report linked to the same evidence. A changed game scoring rule requires the corresponding new Activity Pack Revision. Subjective post-hoc judgments remain separately attributed.

Canonical Replay establishes how recorded inputs produced Room state. It does not reproduce model sampling, certify provider internals, or prove that the evaluator measures the intended construct. Analysis replay also needs evaluator versions and operational/Runner evidence not present in Canonical History.

This boundary follows the existing [Pack contract](adr/0010-activity-pack-v1-and-executable-replay-retention.md), [bundle approval boundary](adr/0014-installable-wasi-free-activity-pack-bundles.md), and [independent Activity Client model](adr/0017-separate-activity-clients-from-packs-and-studio.md). It does not replace them.

## 12. Reference compatibility and unresolved issues

The inspected MindGames starter kit at commit `b7ff02ab4a6b01398d186290e0981c37e31ba3ab` is not a verified complete MG-Ref implementation. Its evaluation example runs eight episodes for each of two environments with random seats and one fixed opponent; its public README retains evaluation-related TODOs. We did not find the advertised full tournament manifests in that snapshot. [Evaluation example](https://github.com/mind-games-challenge/mindgames-starter-kit/blob/b7ff02ab4a6b01398d186290e0981c37e31ba3ab/src/offline_evaluation.py), [README](https://github.com/mind-games-challenge/mindgames-starter-kit/blob/b7ff02ab4a6b01398d186290e0981c37e31ba3ab/README.md)

Before claiming compatibility, resolve:

- which environment source revision is authoritative when paper and code differ;
- exact observations, parser/default behavior, retries, rewards, and termination;
- reproducible initial assignments and randomness across runtimes;
- reference agent availability, executability, configuration, and redistribution terms;
- rating parameters and schedule details sufficient to reproduce the intended protocol.

Until then, label implementations **MindGames-inspired**, or precisely state the pinned source profile they reproduce. Do not equate “same game name” with benchmark equivalence.

Broader open questions include how much external-agent execution can be verified, affordable confirmation budgets, what human-labelled data can validate each rubric, and whether creators can understand the evaluation card without support.

## 13. Delivery sequence and acceptance gates

### First: prove the measurements

Implement one objective-scoring scenario, deterministic baselines, full attempt accounting, a frozen run specification, and an offline report. Add repeated-run execution and a clear uncertainty display.

Acceptance: known-answer fixtures produce expected scores; malformed outputs remain visible; repeated report generation from the same evidence is identical; a second reviewer can trace every headline metric to its numerator, denominator, and source records.

### Next: prove the human strategy comparison

Run the three-player experiment with real API agents. Compare baseline and human-guided entries under a declared matched-block plan. Treat a null or negative effect as a valid result; the platform must not require the preferred strategy to win.

Acceptance: fresh confirmation data, balanced exposure, explicit budget/memory policy, and no substitution of match counts for independent sample counts.

### Then: prove cooperation and creator usability

Add the resource-allocation package, held-out partner tests, and guided template setup. Let an outside contributor add a tested metric or scenario without changing Kernel rules.

Acceptance: ordinary creators can explain the displayed success condition; the report distinguishes team success from individual effect; private data stays out of spectator exports.

### Later: expand evaluation claims carefully

Add calibrated social rubrics, controlled tournaments, and any verified reference-compatibility packages. Consider advanced population analysis only after matchup evidence warrants it.

Acceptance: publish the judge validation and its limits; show missing/unscorable results; preserve old report identities when evaluators change; never market contributor review as scientific validation.

## 14. Bottom line

TextArena gives us a useful platform pattern. The broader literature gives us reasons to avoid treating its leaderboard as the answer to every evaluation problem.

The strongest direction is a community platform with **simple creation and explicit measurement**: creators define goals, participants bring agents, and each scenario produces an auditable scorecard suited to the question. The first credible milestone is a small, repeatable experiment that an outside person can understand and check—not a large catalog or a claim of general social intelligence.
