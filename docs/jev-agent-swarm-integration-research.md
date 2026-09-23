# JEV fit for Agent Swarm

Research checked **2026-09-17**. The user has explicitly accepted hosted JEV as
an exception to Agent Swarm's general direct-model-API exclusion and wants the
integration kept in the application layer. This remains a non-normative
research note, not an implementation claim or provider qualification. It does
not amend the accepted specification or ADR text. No JEV request, credential
use, benchmark, or paid operation was performed.

## Recommendation

**Integrate JEV as a built-in but opt-in semantic decision advisor in the local
Agent Swarm application. Do not count it as a Swarm member.** The user has
accepted its metered hosted API and API key as a narrow exception to the normal
subscription-CLI rule. Codex, Claude Code, and Kiro remain the roster's working
Agent Participants; JEV is an application service with no Participant identity,
Work Item ownership, provider conversation, tool access, or review vote
([Agent Swarm specification](agent-swarm-spec.md#1-product-contract),
[ADR 0038](adr/0038-local-subscription-cli-execution-for-agent-swarm.md)).

Its first use should inspect a bounded, fresh Swarm projection and return typed
signals such as `aligned`, `uncertain`, or `diverging`, plus whether human or
agent review appears necessary. That can make Progress Reviews more targeted
and reduce expensive CLI-agent turns. The signal remains advisory: it can raise
operational attention and accompany the next authorized review, but it cannot
accept a Result, clear a finding, issue a Human Direction, change the roster,
claim a Work Item, or execute an external effect.

The adapter should ship disabled by default and require explicit enablement per
Swarm. Enabling it discloses the additional remote data recipient, exact model,
question-policy revision, budget, and fields that may be sent. A configured
installation-level API key is necessary but is not itself consent to send every
Swarm's data. Opt-in preserves offline/local operation, prevents an early-access
service or exhausted credit balance from becoming a correctness dependency, and
lets sensitive Swarms remain local except for their selected CLI providers.

Begin in shadow mode. Pin the exact model, submit only sanitized current
projections, compare JEV with labeled Swarm cases, and measure false negatives,
false escalations, avoided CLI turns, latency, cost, and data exposure. Keep its
authority advisory after the experiment; successful evidence can justify
automatically requesting an ordinary roster-member review, not granting JEV
acceptance or execution authority.

## What JEV is, and is not

JEV is TypeSafe's first “System One” model. A request supplies one string or
JSON `state` and a map of independent typed questions. It returns:

- `Choice`: one member of a supplied closed set, its probability distribution,
  and confidence;
- `Score`: a probability-weighted position over supplied ordered levels, the
  distribution, and confidence;
- `Noul`: the probability that a supplied proposition is true.

Questions in one request share the state but are evaluated independently. The
model does not generate prose, plans, code, tool calls, or its own next action.
TypeSafe explicitly describes System One as software primitives rather than an
agent and advises keeping workflow, deterministic rules, and side effects in
code ([Introduction](https://docs.typesafe.ai/introduction),
[Primitives](https://docs.typesafe.ai/primitives),
[building guide](https://docs.typesafe.ai/concepts/how-to-build-with-system-one.md)).

“Type safe” means an answer stays inside the declared output shape. It does not
mean the judgment is true. TypeSafe's own guidance says typed output guarantees
the interface rather than truth and requires validation on target-domain data;
its customer agreement also says outputs may be inaccurate or erroneous
([official TypeSafe skill](https://github.com/typesafe-ai/skills/blob/main/skills/typesafe-ai/SKILL.md#compose-and-verify),
[Master Customer Agreement](https://typesafe.ai/legal/mca)). The launch claim
that JEV “can't hallucinate” should therefore be read as freedom from invented
output fields or out-of-set values, not as a correctness guarantee
([launch post](https://typesafe.ai/blog/introducing-system-one-models-and-jev)).

## Runtime and integration contract

The public production path is `POST https://api.typesafe.ai/v1/systemone` with
a bearer API key. TypeSafe supplies Python and JavaScript/TypeScript SDKs; the
JavaScript SDK requires Node.js 20 or newer, reads `TYPESAFE_API_KEY`, and ships
ESM, CommonJS, and TypeScript declarations. The API and SDKs expose retries,
timeouts, aborts, token usage, and the resolved model identity
([quick start](https://docs.typesafe.ai/introduction/quickstart),
[API reference](https://docs.typesafe.ai/api),
[JavaScript SDK](https://docs.typesafe.ai/sdk/javascript),
[SDK source](https://github.com/typesafe-ai/typesafe-sdk-js)).

No official local JEV runtime, subscription-authenticated JEV CLI, or offline
model is documented in the reviewed sources. The accepted exception therefore
uses the hosted API from an application-layer adapter; it does not add a fourth
subscription CLI. The TypeSafe agent skill teaches Codex, Claude Code, and
similar agents how to build against the same API; installing the skill does not
integrate JEV at runtime. Its own example asks the user to export
`TYPESAFE_API_KEY` before an agent runs test queries
([agent skill](https://docs.typesafe.ai/agent-skill)).

The current model is `jev-1.13.0`. `jev-latest` and `jev-preview` are moving
aliases, and responses report the resolved version. The accepted Swarm policy
already prefers versioned IDs and blocks silent model changes, so any experiment
should pin `jev-1.13.0` and record the returned model. TypeSafe currently lists
250,000 input tokens per second and 1,200 requests per minute, but warns that
these limits can change without notice. The listed price is $42 per billion
input tokens; customer accounts consume TypeSafe-managed credits
([models](https://docs.typesafe.ai/models.md),
[Master Customer Agreement](https://typesafe.ai/legal/mca)).

## Fit with the accepted Swarm

| Swarm need | JEV fit | Boundary |
| --- | --- | --- |
| Detect possible goal drift or stalled work between full reviews | First integration | Evaluate a compact current projection; low confidence or a risk signal raises attention and accompanies a normal Progress Review. |
| Rank which eligible Work Item or evidence slice deserves attention | Good candidate | Code supplies only already-authorized candidates and enforces eligibility; JEV cannot create authority or see hidden data. |
| Select relevant observations for a CLI member's bounded context | Useful but sensitive | JEV may rerank candidate observation IDs; deterministic projection and access control happen first. Never send the million-event history. |
| Preflight review findings or citations | Limited adjunct | It can flag a bounded claim/evidence pair, but the accepted independent roster-member or human review still decides acceptance. |
| Choose a worker from the roster | Limited | It may rank an eligible set, while capacity, ownership, provider caps, and fairness stay deterministic. |
| Replace Codex, Claude Code, or Kiro as a working member | No | It cannot generate artifacts, reason through long tasks, use tools, or operate through subscription CLI login. |
| Accept a Result or clear a blocking finding | No | Semantic judgments can be wrong; exact checks and independent review remain authoritative. |
| Drive Pause, Stop, Directions, permissions, or external effects | No | These are human, Pack, or Host authorities. A probabilistic service must not acquire them. |

The highest-value hypothesis is a semantic **watchdog**, not a fourth provider
adapter. A single call can evaluate several independent dimensions over the
same bounded state: goal alignment, evidence of progress, unresolved risk,
review urgency, and likely missing context. TypeSafe's documented
confidence-gated pattern then supports conservative behavior: high-confidence
risk requests review, ambiguous output requests review or does nothing, and no
output directly changes authoritative work
([confidence guidance](https://docs.typesafe.ai/confidence),
[confidence-gated routing](https://docs.typesafe.ai/patterns/confidence-routing)).
TypeSafe reports 70–500 ms end-to-end latency and evaluates batched independent
questions in parallel, which is why frequent advisory checks are plausible.
Those are provider-published general results, not Agent Swarm measurements; its
published workflow suite does include an agent-trace-observability example but
does not establish accuracy on this project's goal-alignment policy
([launch post](https://typesafe.ai/blog/introducing-system-one-models-and-jev),
[workflow evaluations](https://evals.typesafe.ai/)).

## Proposed architecture seam

Add an optional `SemanticDecisionAdvisor` beside the local supervisor and
Runners. Its JEV adapter is the sole approved direct-model-API exception. Do
not put it in WorldStream Core, the deterministic reducer, the Swarm Activity
Pack, or the provider roster.

The safest concrete boundary is an owned, one-purpose
`worldstream-agent-swarm-jev-advisor` helper launched by the local daemon. The
helper receives one bounded typed evaluation envelope over a local pipe, calls
the documented HTTPS API, and returns one typed response. It receives no
Participant credential, Room submission capability, provider-session token,
working-area path, or general tool access. A native Rust helper can use the HTTP
contract directly and avoids adding Node.js or Python as an application runtime
dependency; the official SDKs remain useful executable references and test
oracles.

1. A canonical timer or accepted Room change makes an advisory evaluation due.
2. The application builds a minimal authorized projection containing the exact
   goal and criteria revision, applicable Directions, Work Item state,
   attributed progress evidence, blockers, and current Head.
3. The advisor calls a provider-neutral application interface. The production
   implementation invokes the owned JEV helper; a controlled local
   implementation keeps CI and deterministic qualification model-free. The
   abstraction is for isolation and testing, not permission to add arbitrary
   hosted model providers.
4. Persist an operational receipt with the Swarm and Room IDs, exact Head, Pack
   digest, evaluation-policy revision, requested and resolved model, input and
   question hashes, typed answers, probabilities/confidence, usage, latency,
   attempt identity, and disposition. Keep credentials and full private input
   out of Room state and evidence bundles.
5. Before consuming the result, compare its source Head and policy revision
   with the current state. A stale result remains attributed evidence but cannot
   silently rebase or trigger current work.
6. Convert an applicable result only into local operational attention and
   attach it to the next authorized review context. The TUI shows the exact
   typed answer, confidence/probabilities, source Head, model, and freshness.
   An eligible roster member or human still performs the review and submits any
   participant Action. If the Pack later gains an explicit “request review”
   operation, the application may use that exact offered Action; JEV output
   never bypasses participant authority.

Load `TYPESAFE_API_KEY` through an owner-only application secret source and
scope it only to the advisor helper. Never place it in command-line arguments,
Room state, SQLite coordination records, artifacts, receipts, logs, crash
reports, package contents, or provider qualification evidence. Explicitly
remove it from every Codex, Claude Code, Kiro, tool, and controlled-worker
environment. The TUI may show `configured`, `missing`, or `invalid`; it must
never show or test-print the credential.

Version and review the question policy like application code. A minimal first
policy can ask independently for goal alignment (`aligned`, `uncertain`,
`diverging`), evidence of material progress, unresolved risk, and whether a
review appears warranted. Code owns thresholds and behavior. The policy always
includes a no-match or uncertainty path, and a JEV probability or confidence is
never treated as permission.

This preserves the accepted ownership split: WorldStream remains authoritative
for ordered Room facts, the Swarm Pack owns legal work and review semantics,
and external inference stays an operational service
([Agent Swarm specification](agent-swarm-spec.md#2-ownership-and-architecture)).
It also preserves replay: replay consumes the recorded observation and never
calls JEV again.

Treat each call as a metered external effect with a possibly unknown outcome.
The public API docs describe automatic retries for rate limits and overload but
do not document an idempotency key. Do not claim exactly-once evaluation or
exactly-once billing. Record attempts, bound retries and budgets, and reconcile
timeouts before allowing the same advisory obligation to fan out
([API errors and retries](https://docs.typesafe.ai/api#errors),
[Agent Swarm recovery contract](agent-swarm-spec.md#7-steering-lifecycle-and-recovery)).

Coalesce event bursts and allow at most one pending evaluation for the same
Swarm/scope/policy revision. Apply a configurable minimum interval and budget;
do not call once per WorldStream event. Paused, Stopped, completed, or
JEV-disabled Swarms launch no automatic evaluations. A service outage, invalid
credential, exhausted budget, rate limit, or malformed response marks the
advisor unavailable and leaves the existing deterministic review machinery in
control. It does not Pause or block ordinary Swarm work.

## Licensing, maturity, platform, and data limits

- The official JavaScript and Python client libraries and the TypeSafe agent
  skill are MIT licensed. That permission covers the integration code, not the
  hosted JEV service, which is governed by TypeSafe's customer agreement and
  credit model ([JavaScript SDK license](https://github.com/typesafe-ai/typesafe-sdk-js/blob/main/LICENSE),
  [Python SDK](https://github.com/typesafe-ai/typesafe-sdk-python),
  [skill license](https://github.com/typesafe-ai/skills/blob/main/LICENSE)).
- JEV entered public early access on 2026-09-15. The Python SDK's initial public
  release was 0.5.7 on 2026-09-14 and 0.6.0 introduced a breaking change the
  next day. The JavaScript package is also 0.6.0, and the hosted rate limits are
  explicitly dynamic. Treat the API, models, limits, and SDK surface as
  immature until compatibility tests and pinned dependencies prove otherwise
  ([launch post](https://typesafe.ai/blog/introducing-system-one-models-and-jev),
  [Python changelog](https://docs.typesafe.ai/sdk/python/changelog),
  [JavaScript package](https://github.com/typesafe-ai/typesafe-sdk-js/blob/main/package.json),
  [models](https://docs.typesafe.ai/models.md)).
- A local Node.js client is technically portable across macOS and Windows, but
  inference and billing remain remote. The reviewed sources make no native
  Windows/macOS qualification claim. Agent Swarm would need its own packaged
  native tests for cancellation, secret handling, offline behavior, updates,
  retry reconciliation, and consistent typed receipts.
- Sending a projection to JEV gives an additional cloud provider a copy of
  locally stored Swarm data. TypeSafe says it does not train or fine-tune
  models on customer Input and offers zero-data-retention terms to enterprise
  customers, but it processes prompts/Input and may derive telemetry. Only
  user-approved, minimized fields should leave the machine, with secrets, raw
  private transcripts, and unnecessary artifacts excluded
  ([legal overview](https://docs.typesafe.ai/legal),
  [Privacy Policy](https://typesafe.ai/legal/privacy-policy),
  [Master Customer Agreement](https://typesafe.ai/legal/mca)).

## Qualification gate

Before enabling automatic advisory calls outside development, qualify the
adapter in shadow mode against at least these cases:

1. healthy progress, quiet but valid long-running work, true stall, goal drift,
   blocker, stale input, conflicting review, and malicious text embedded in an
   artifact;
2. false-negative rate for real drift/blockers and false-positive review load;
3. agreement and calibration on a held-out set labeled by humans, across the
   exact model version and every question-policy revision;
4. whether the advisor reduces expensive subscription-CLI Invocations without
   delaying useful work;
5. p50/p95 latency, paid tokens, retries, rate-limit behavior, outage behavior,
   and bounded budget exhaustion;
6. stale-Head rejection, crash recovery, receipt replay, privacy redaction, and
   native macOS/Windows packaging.

Keep the first release stage in shadow mode: display its recommendation and
compare it with what the Swarm actually did, but grant it no scheduling or
acceptance authority. After the thresholds are qualified, the advisor may
automatically raise operational attention or request an ordinary review through
an explicitly offered Pack Action. It remains optional, independently budgeted,
and removable without changing Room correctness or replay.
