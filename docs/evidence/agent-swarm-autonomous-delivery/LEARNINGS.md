# Learnings and next experiments

The production delivery loop now works for the bounded case. The model selected integration, actual checking, independent review and delivery without the development assistant continuing those decisions. WorldStream supplied accepted history and authority throughout.

## 1. Coordination cost is the clearest performance issue

Twelve of 22 Invocations were planning; six more only proposed or claimed work. Planning consumed 186.889 summed native seconds. All recorded Swarm turns occupied 415.740 seconds of the 745.240-second trial after accounting for overlap. The other 329.500 seconds include admission, process/validation overhead, polling, checks and Room operations; this run does not isolate their individual costs.

Next: instrument those stages separately, then evaluate model-authored metadata batches that preserve sequential Room validation. Keep decomposition and strategic choices with the model. Compare equivalent serial and parallel trials before claiming an overall speedup. The observed 84.202-second overlap proves concurrency, not a benchmark advantage.

## 2. Precise schema feedback is still useful

One planning response placed `reason` in a dispatch step instead of the decision. The coordinator rejected it and the model corrected the next response. This is within-run recovery, but the retained category was only `provider_output_invalid`.

Next: return bounded field-level schema feedback and evaluate supported structured outputs. Preserve the distinction between definite rejection and an uncertain Action response; never replay uncertain effects as a repair strategy.

## 3. Test Contributions should participate in live acceptance

Agent B's independently authored tests were available to integration and judging. They were not executed by the live acceptance gate. All 26 passed in a separate post-run sandboxed replay, with source and tests unchanged.

Next: explicitly authorize an isolated test executor, bind test and Candidate versions, and run both fixed acceptance checks and authored tests before independent review. Generated tests must not replace the independent acceptance criteria.

## 4. Demonstrate live correction before broader autonomy

The first Candidate passed. This run therefore does not establish model-led repair after a real failed check or a blocking judge finding. Controlled tests exercise those continuation paths.

Next: run a separate, predeclared fault case with unchanged criteria. Require a failed check or independent finding, model-authored revision, fresh checks/review, explicit finding resolution where needed, and accepted delivery without evaluator intervention. Multi-file work should follow with isolated workspaces and explicit conflict handling.

## 5. Learning and JEV remain explicit next steps

The development agent fixed the judge-session bug and analyzed these logs. That is not evidence that the Swarm modifies its own code or learns across runs. A useful learning increment would store a model-authored retrospective as versioned advice and evaluate it on held-out goals before adoption.

JEV was not used here. [ADR 0043](../../adr/0043-use-jev-as-an-application-layer-swarm-advisor.md) places it at an opt-in advisory boundary, initially in shadow mode, without a review vote or Result authority. A comparison experiment could test whether its assessments identify coordination waste or weak evidence earlier than the baseline. The independent native judge demonstrated here remains a separate role.
