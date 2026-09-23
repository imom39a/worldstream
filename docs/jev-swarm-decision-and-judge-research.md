# JEV for Swarm decisions and contribution judging

Research checked **2026-09-22** against live TypeSafe documentation. This note
extends [the earlier integration research](jev-agent-swarm-integration-research.md)
with a concrete first judge policy; it does not change accepted authority or
claim an implemented or qualified integration. No credentials, inference calls,
or paid operations were used.

## Recommendation

Use JEV as an optional **criterion-level semantic evaluator** and **decision
advisor**. Start with shadow assessments of a proposed contribution and its
evidence, plus attention signals for ordinary Progress Reviews. Later, evaluate
advisory ranking of already-eligible alternatives. This fits
[ADR 0043](adr/0043-use-jev-as-an-application-layer-swarm-advisor.md): JEV produces
Swarm Assessments in the application layer, holds no Participant identity or
review vote, and cannot accept a Result, clear a finding, issue a Direction, or
dispatch work. Calling this a “judge” describes its evaluation task, not an
authority over the Swarm.

Roster agents generate plans, contributions, evidence and explanations. Code
selects authorized input and performs exact checks. JEV answers narrow semantic
questions about those inputs. A roster member or human performs the ordinary
review and explains any consequential decision. JEV is a typed decision model;
it does not generate a critique, fix, plan, or evidentiary explanation
([System One](https://docs.typesafe.ai/concepts/system-one),
[coding agents](https://docs.typesafe.ai/introduction/coding-agents)).

## Current implementation and integration seams

The checkout already contains the accepted advisor design and
[implementation slice 19](agent-swarm-ticket-plan.md), but the Swarm crate and
Pack contain no JEV client, assessment configuration, receipt store, or advisor
UI. The separate [Minesweeper client](../examples/minesweeper/agents.py) calls
JEV to select cells; it is not an Agent Swarm integration or a qualified judge.

The first implementation can stay in the application layer:

| Seam in current code | Proposed addition |
| --- | --- |
| [`CoordinatorService::cycle`](../crates/worldstream-agent-swarm/src/coordinator_service.rs), lines 503–566 | Enqueue bounded assessment work from a fresh authorized observation, outside the dispatch-critical path; coalesce duplicate scope/policy work and honor execution controls |
| [`SwarmObservation`](../crates/worldstream-agent-swarm/src/backend.rs), lines 83–96 | Build a separate minimized evaluation envelope; this observation explicitly excludes artifact bytes, so judging needs authorized content extraction rather than references alone |
| [`CandidateReview` and `provider_prompt`](../crates/worldstream-agent-swarm/src/coordinator.rs), lines 149 and 2941 | Bind advice to candidate, criteria, input digests and exact Room Head, and attach applicable receipts as clearly attributed operational evidence in an eligible reviewer's context |
| [`CoordinatorServiceView`](../crates/worldstream-agent-swarm/src/domain.rs), line 139, and [TUI](../crates/worldstream-agent-swarm/src/tui/mod.rs), line 2322 | Show assessment answers, uncertainty, model, policy, source Head and freshness beside the ordinary review state |
| [Owned process execution](../crates/worldstream-agent-swarm/src/execution/process.rs), line 220 | Host a dedicated JEV helper with a separately validated request, isolated credential, bounded timeout and independent budget |

The helper receives only approved text and typed fields, never a working-area
path or authority to retrieve more files. Artifact extraction belongs to the
authorized application side. Preserve the full evidence binding in the local
receipt; identifiers and hashes need not all be sent to the model if they add
no judgment context. Recheck the exact Head and evidence binding before using
advice. A review claim or unrelated Room change can advance the Head while an
assessment runs; retain the old receipt as stale and evaluate fresh state when
appropriate instead of silently rebasing it or delaying ordinary work.

Three implementation boundaries need explicit attention:

- [`recordReview` and `acceptResult`](../packs/agent-swarm/src/workflow.ts),
  lines 719 and 784, retain independent review, exact passing checks, current
  material and unresolved-finding gates. JEV output must not be converted
  automatically into those authoritative records.
- The current [Action descriptors](../packs/agent-swarm/src/schemas.ts),
  lines 124–153, expose no `request_progress_review` Action. First attach advice
  to existing review opportunities and show operational attention. Creating an
  extra canonical review obligation requires an explicit supported authority
  path; do not invent an Action or fabricate a blocker to trigger one.
- The current [provider environment scrub list](../crates/worldstream-agent-swarm/src/execution/provider/mod.rs),
  line 895, does not list `TYPESAFE_API_KEY` or the demo's `JEV_API_KEY`.
  Isolate the new secret and cover inherited environments, provider probes,
  worker tools and helper cleanup in qualification before enabling it.

Candidate judging adds a concrete policy to the existing advisor slice; it is
a proposed implementation sequence, not a change to the accepted specification
or a claim that its packaging and review prerequisites are qualified.

## Proposed first question policy

Build a small state cut containing the exact Room Head, goal and acceptance
criteria versions, applicable Directions, one candidate contribution version,
explicitly authorized excerpts or diffs pinned to their Artifact digests, and
deterministic check receipts. Metadata alone cannot establish evidence support.
Questions must identify their relevant state fields explicitly; question IDs are only response
keys, not model instructions ([HTTP API](https://docs.typesafe.ai/api)).

| Judgment | Primitive and proposed rubric | Application use |
| --- | --- | --- |
| Evidence support for one acceptance criterion | `Choice`: `supported`, `contradicted`, `insufficient_evidence`; distinguish evidence of behavior from an agent's assertion that it works | Show which criterion needs ordinary review; never mark the criterion accepted |
| A specific semantic violation | One `Noul` per explicit condition, such as “Does the proposed approach conflict with this Direction?” | Surface each risk separately; one serious flag cannot be averaged away |
| Candidate goal fit | `Score` with concrete ordered levels: unrelated; addresses an adjacent concern; partly addresses the stated goal; directly addresses it | Compare supplied proposals on a shared rubric; retain raw dimension scores |
| Attention need | `Choice`: `no_new_attention`, `possible_drift`, `conflicting_evidence`, `insufficient_context`, each defined against supplied progress facts | Add context to the next authorized Progress Review |

This is a proposed policy to evaluate, not a validated prompt. A useful concrete
example is a contribution claiming “the app works offline,” accompanied only
by a compilation receipt. Compilation is an exact success in code; evidence
that offline behavior works is still insufficient. A prose assurance must not
silently become observed behavior.

For grounded checking, code first verifies evidence identity, source existence,
version freshness, and exact quoted spans. Only then ask whether the supplied
context supports the claim. This adapts TypeSafe's
[citation-check recipe](https://docs.typesafe.ai/cookbooks/citation_check).
A missing exact quote should be labeled a quote-match failure in our UI; a
paraphrase or truncation alone does not establish fabrication. Text assessments
cannot substitute for executing tests, inspecting a binary, or viewing a UI.

For advisory decision ranking, code supplies only eligible candidate IDs and
JEV evaluates each against the same independent dimensions. Code may combine
preference dimensions with transparent weights; hard constraints and serious
violations remain separate. Include a no-suitable-candidate path. A relative
winner does not establish that any candidate is good enough. Compare goal fit,
evidence relevance, and unresolved uncertainty; keep capacity, dependency
readiness, arithmetic, authority, and final scheduling deterministic
([composite scoring](https://docs.typesafe.ai/patterns/composite-scoring),
[Score](https://docs.typesafe.ai/primitives/score)).

## Current provider contract and limits

The documented model remains `jev-1.13.0`; pin it and record the resolved model.
It takes text/JSON, not images, audio or video. The limits are 64k tokens for
the entire request and 32k for state plus the longest question. Current listed
pricing is **$0.042 per million input tokens**, with output tokens free; rate
limits can change. At that rate, 1,000 evaluations of 10,000 billed input
tokens each would cost about **$0.42**, excluding retries. This is arithmetic,
not measured Swarm consumption or latency
([models](https://docs.typesafe.ai/models)).

Independent questions share state and run in parallel. Batch dimensions when
they use the same evidence; a question cannot consume another question's answer
from that same request. Retrieve new evidence or construct new options before
a dependent second request
([building guide](https://docs.typesafe.ai/concepts/how-to-build-with-system-one)).

Choice and Score confidence describes concentration of their answer
distributions. It is not the measured probability that the whole contribution
is correct. Noul returns the probability of its yes/no proposition and has no
separate confidence. Set thresholds from Swarm labels and consequences rather
than copying cookbook values
([confidence](https://docs.typesafe.ai/confidence)).

The provider explicitly documents weak numeric precision, unreliable counting
and date comparisons, reduced accuracy with multi-hop indirection or irrelevant
context, and vulnerability to adversarial text in state. It also warns that
equivalent-looking Noul and Choice questions need not produce equivalent
probabilities. Keep one formulation per evaluated condition, use short grounded
inputs, and test injected instructions within contributions
([JEV 1.13 limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13)).

## What the published examples establish

The [citation-check](https://docs.typesafe.ai/cookbooks/citation_check),
[extraction-cascade](https://docs.typesafe.ai/cookbooks/sde_cascade), and
[reranking](https://docs.typesafe.ai/cookbooks/rerank_typesafe) examples use
`jev-1.12`, not the current 1.13. Citation checking covers eight prepared
citations; the cascade reports internal extraction results and a deliberately
hard-coded single-item example; reranking measures legal-passage retrieval.
Their measured results support these workflow patterns, but do not establish
Swarm judge accuracy, reduced CLI turns, or replacement of independent review.

## Concrete evaluation and rollout

1. Assemble an initial 100–200 labeled cases spanning coding and non-coding
   contributions, evidence omissions, genuine contradictions, quiet valid
   progress, drift, stale inputs and injected instructions. This is a pilot
   size, not a statistical guarantee. Include confidently written but unsupported
   claims and correct contributions with modest wording.
2. Have two reviewers independently label criterion support and review need;
   adjudicate disagreements without showing them JEV output. Preserve genuinely
   insufficient evidence as a distinct label. Split by goal or task family so
   near-duplicate contributions cannot leak between development and held-out
   evaluation.
3. Freeze model, input selection and policy before the held-out run. Measure
   per-condition false negatives, false alarms, abstention/coverage, probability
   calibration, and agreement with adjudicated labels. For ranking, compare
   reviewer preference and usefulness of the top candidate; permute candidate
   order and remove provider identity to test avoidable bias.
4. Compare the existing deterministic checks and review process with the same
   process plus JEV advice. Record incremental defects found, review burden,
   avoided CLI turns, time to useful review, token use, retry count and p50/p95
   latency. A duplicate assessment adds little value even when accurate.
5. Exercise application failure handling with a fake adapter: stale Head or
   candidate version, malformed response, cancellation, timeout, budget
   exhaustion and unavailable service. Keep assessments operational, replay
   their receipts without inference, and preserve ordinary work when advice is
   unavailable.

Start disabled by default, explicitly enabled per Swarm, with a bounded budget
and shadow display. Keep the API key in the isolated helper and minimize remote
input as required by ADR 0043. Subsequent evidence can justify operational
attention or an ordinary review request through existing authority; it cannot
turn a high-confidence answer into a review vote or Result acceptance. A judge
with that broader authority would require a separate domain and ADR decision.
