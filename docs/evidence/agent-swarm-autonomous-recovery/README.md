# Live autonomous recovery experiment

**Latest outcome: unsuccessful; recovery was not demonstrated.** Live attempt
4b accepted one baseline-first Work Item, then stopped at the unchanged harness
command deadline. It produced no Candidate, Check, Contribution, review,
Result, or delivery, so it did not test failure-led repair. The retained
[attempt 4b report](attempt-4b/summary.json) and
[evidence export](attempt-4b/) record the stopped run; the
[bounded timeout diagnosis](attempt-4b/timeout-diagnosis.md) separates observations
from possible causes. The preceding
[attempt 4 preflight note](attempt-4-preflight.md) is a separate setup-only
failure: it used a Python interpreter without `blake3`, wrote identities only,
and did not start a managed run or invoke a model. The cause of attempt 4b's
command timeout has not been established.

**Attempt 3 outcome: unsuccessful.** The live Swarm decomposed the task and authored
implementation and test Contributions concurrently, but did not reach an
accepted Candidate, production Check, independent review, or delivery.
It also authored the repair before reproducing the baseline, contrary to the
[predeclared ordering requirement](TEST-DESIGN.md).

GPT-6 Sol implemented and executed the evaluation; GPT-6 Luna independently
audited the evidence. The measured native Swarm retained the previously approved
`gpt-5.6-sol` / `medium` configuration. The development subagents and measured
Swarm are separate. JEV was not used.

## What ran

The task started from the prior delivered CSV tool with exactly one call removed:
`_validate_quoting(text)` in `parse_records`. The frozen 4,826-byte source has
SHA-256 `51963d5fd5af026ef5c70a0d0889d04c9e69a0f41a8246fda4db560e8f7f206a`.
The unchanged checker independently demonstrated 25/26 passing cases before
launch, failing `quote_in_unquoted_field`. That preflight is **not** a Check
recorded by the live Swarm.

The goal required an unchanged first Candidate, an actual failed Check, then
model-authored revision, fresh passing checks, independent review and delivery.
The evaluator initialized the run and drove service ticks; it supplied no
post-setup Room Actions or delivery plans and did not steer the live planner.

| Attempt | Outcome | Elapsed |
| --- | --- | --- |
| [1](attempt-1/summary.json) | Setup stopped: 7,045-byte goal exceeded the managed 4 KiB limit | 73.014 s |
| [2](attempt-2/summary.json) | Setup stopped: configuration strings contained literal newlines | 70.711 s |
| [3](attempt-3/summary.json) | Autonomous work ran; Candidate submission became uncertain and execution stopped | 732.524 s |
| [4 preflight](attempt-4-preflight.md) | Setup-only Python dependency failure; no managed run or model action | — |
| [4b](attempt-4b/summary.json) | One baseline Work Item accepted; harness command timeout before a Candidate or Check | 560.892 s |

The same defective source, checker, criterion and execution policy were retained
through the setup corrections. Before attempt 3, the complete corrected setup
[passed validation](setup-only-preflight.json) through a fresh controller and the installed Pack. All
managed attempts stopped their processes and retained unchanged measured file
identities; attempt 4 preflight started no managed processes. Attempt 3's pre-run
and live declarations match.

## What the live evidence establishes

| Measurement | Attempt 3 |
| --- | --- |
| Model-generated Work Items | 3 |
| Accepted Contributions | 2: proposed repair and adversarial tests |
| Native authoring overlap | 69.862 s |
| Model invocations | 19: 9 planning, 3 proposal, 3 claim, 4 work attempts |
| Accepted Candidates / Checks / Reviews / Results | 0 / 0 / 0 / 0 |
| Authoritative final Room sequence | 10 |

At Room sequence 9, Agent B submitted repaired source restoring the missing
validation call. At sequence 10, Agent C submitted its test Contribution. This
establishes concurrent, separate authoring, but fails the frozen experiment's
repair-after-reproduction requirement. The tests were not executed as an
acceptance gate in this run.

Correction: the first integration response was rejected locally as
`provider_output_invalid` because its `expected_digest` used `sha256:`. The
artifact field is supported, but this field accepts only a canonical `blake3:`
digest; the coordinator reports that format through its strict proposal
decoder. The subsequent
response produced bytes exactly matching the defective baseline, but supplied
`contribution_refs: []`. The Pack requires at least one Contribution reference.
The planner had explicitly requested the empty list because it did not intend
to incorporate the already-authored repair or tests into the unchanged baseline.

The corresponding Action, `1XAQD9FPQET34QDEEF7MKFT7P5`, remained
`submission_uncertain`. Independent read-only inspection found it in neither
the Room's committed transitions nor semantic receipts. The Room has no
Candidate. The payload's schema violation is established; the retained evidence
does **not** prove which transport or rejection response caused uncertainty.
No Action was retried, and no recovery or delivery is claimed.

## Changes and validation

Added an instructed recovery harness, frozen starter, declaration and evidence
export, plus an offline verifier that checks exact artifacts, check output,
native model provenance, failure feedback and fresh review. Setup now carries
the source as bounded JSON string literals that decode losslessly. Existing
delivery defaults remain unchanged; no production Rust or Pack behavior was
changed for this experiment.

The focused Python suite passed **24 tests**; Ruff checks and formatting passed.
Native-output compatibility was checked against the prior real delivery run.
The complete recovery verifier was not run on attempt 3 because no successful
delivery report existed; passing verifier unit tests do not make this trial pass.
The experiment's repair-before-reproduction clause also needs the semantic
chronology audit described above; mechanical artifact joins alone are insufficient.
The failed-attempt exports have separate artifact manifests.

See [metrics](metrics.json), [Room evidence](attempt-3/room-summary.json),
[selected native decisions](attempt-3/selected-intents.json),
[final service outcome](attempt-3/adaptive-final.json), and
[learnings and next changes](LEARNINGS.md), and the
[production validation change and rerun](IMPROVEMENT-AND-RERUN.md). Raw credentials and prepared process
environments are excluded from the export.

The [previous successful delivery run](../agent-swarm-autonomous-delivery/README.md)
remains separate evidence. This new experiment does not establish failure-led
repair, independent judging plus delivery, speedup, or cross-run self-improvement.
