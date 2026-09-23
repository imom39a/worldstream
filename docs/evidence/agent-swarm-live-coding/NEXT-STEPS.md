# From this evaluation to autonomous delivery

Update: the bounded production delivery loop below is now demonstrated by the [autonomous run](../agent-swarm-autonomous-delivery/README.md). The live repair case and authored-test acceptance gate remain outstanding; see the [updated learnings](../agent-swarm-autonomous-delivery/LEARNINGS.md).

WorldStream remains the kernel: accepted events define ownership, revisions, dependencies, Contributions, Candidates, Reviews and Results. The application supplies reasoning, process supervision and external effects. A model can change its proposed strategy; it cannot rewrite accepted history or bypass authority rules.

## 1. Close the production delivery loop

Extend the production planner's retained options beyond ordinary Contributions. It should be able to propose integration work, request exact checks, request an independent review, respond to failures, and present a ready Result for acceptance and writeback through an explicitly authorized coordinator policy. Preserve existing separation between authors, reviewers and finding resolvers. Do not grant an LLM the Human coordinator's authority merely to remove the evaluation driver.

Acceptance test: run this same goal with no delivery plans staged by the evaluation driver. Require exact check/review bindings, an accepted Result, applied writeback, and full cleanup. Add a second case where a real failed check or review causes a model-authored revision and a fresh independent review.

Execute agent-authored test Contributions within the live check flow, bound to their exact artifact versions and the Candidate being judged. Attempt 16's authored tests passed only in the supplemental post-run replay; the live gate used the fixed evaluator checks. Preserve both sources of evidence and do not allow generated tests to replace the independent acceptance criteria.

## 2. Make the model/action boundary complete

The current prompt includes action descriptions and schema digests, but not a complete machine-enforced output schema. Validate the entire proposed payload against the resolved action schema before submission; provide precise errors to the model. Evaluate native structured outputs with supported exact schemas. Preserve rejected output as evidence.

Also retain definite Runtime protocol rejections separately from unknown transport outcomes. A confirmed `invalid_payload` response can drive correction. An uncertain Action must retain its original identity and require exact reconciliation. The resource-reference guard added in this evaluation fixes one observed case, not this general adapter problem.

Acceptance tests: malformed nested references, unknown fields, invalid enum values, malformed JSON, a definite Runtime rejection, and a lost response after an accepted mutation. None may produce duplicate effects or silently consume work.

## 3. Reduce coordination overhead

Measure model time, executable validation, supervision and Room commits separately. Today proposal and claim each require planning plus a worker turn, and planning waits for the complete selected batch. Investigate typed metadata proposals authored directly in one reasoning turn, cached validation tied to stable executable identity, and planning independent work while other native attempts remain active.

Keep task decomposition with the LLM. The scheduler should explain available capacity and dependencies, validate choices and supervise execution. Compare equivalent serial and parallel trials before claiming a speedup; native overlap alone is insufficient.

## 4. Make learning inspectable

Use retained event history and check/review evidence to produce a model-authored retrospective: which decisions failed, what changed, and which later evidence supports the change. Store proposed strategy guidance as versioned, non-authoritative advice. Test it on held-out tasks before adopting it as a default. Keep evaluation criteria and permission boundaries outside self-modifying model advice.

This is strategy adaptation and memory. It is not model-weight training. The fixes made by the development agent during this evaluation are not evidence of autonomous cross-run learning by the Swarm.

## 5. Add JEV at the existing advisory boundary

Follow [ADR 0043](../../adr/0043-use-jev-as-an-application-layer-swarm-advisor.md): start with explicit opt-in, bounded, shadow-mode assessments of minimized current projections. Bind each assessment to the exact Room head and policy. Measure whether it detects stalled work, weak decomposition or overlooked review risks better than the baseline.

A JEV assessment may inform an ordinary agent decision or request review through existing authority. It does not acquire a review vote, work ownership or Result acceptance authority. A separate proposal would be needed to change that design.
