# Genuine local Codex prequalification probes

Recorded 2026-09-22 on macOS arm64 using installed native `codex-cli 0.149.0`,
provider-managed ChatGPT sign-in, and the user's explicitly selected
`gpt-5.6-sol` / `medium` setting with moving-alias acknowledgement.

These are bounded **synthetic planning probes** through the real native process
guard. They did not dispatch Room Actions, run a live multi-agent Swarm, implement
the requested CSV converter, or qualify a provider for admission. The production
gate remains blocked. The controlled end-to-end Swarm evidence is a separate
artifact and must not be conflated with this provider evidence.

## Observations

| Probe | Outcome |
| --- | --- |
| Attempt 1, fresh | Genuine model response in 24.499 s; useful parser, formatter, and independent-test instructions. This initial harness offered three simultaneous metadata proposals, which the product would reject. Retained as abstract reasoning evidence only. |
| Attempt 1, resume | Guard failed closed after 11.516 s. Provider history confirms the new turn was interrupted with no assistant answer accepted. |
| Attempt 2, fresh | Genuine model response in 21.365 s using the product's planning envelope and one metadata proposal. Selected a bounded parser task. |
| Attempt 2, resume | Genuine model response in 23.357 s on the exact same provider thread. Selected formatter repair, skipped the passing parser, and required a new candidate ID/digest before checks. |

The primary successful evidence is the actual
[initial provider transcript](attempt-2/initial-transcript.json),
[resumed provider transcript](attempt-2/repair-transcript.json), and
[attempt-2 report](attempt-2/report.json). Both turns reported the exact selected model/effort. Their
observed item types contain only user messages, agent messages, and (in the
resumed turn) reasoning. No tool item was observed. This is an observation of
these turns, not proof that tools were inaccessible.

The resumed decision chose repair alone, a legitimate conservative strategy.
The harness preferred repair plus parallel preparation of regression tests, so
the historical `expected_next_work_selected` field remains **false**. This field
measures equality to a harness preference; it is neither a quality oracle nor
evidence of a capability failure. The repair follows the failure evidence and
requires a new candidate before checks. Preparing tests in parallel was an
optional opportunity. The current script calls this metric
`matches_harness_preferred_next_work` and states that limitation explicitly;
the original observed assessments are preserved. This one sample cannot
establish general planning quality or throughput.
The initial decision's reason also treated test proposal as dependent on parser
work more strongly than necessary. More precise separation of independent test
authoring from later candidate checking is a useful next prompt experiment.

## What the live failure improved

Codex emits `thread/tokenUsage/updated` for the previous completed turn after
resuming a thread. The first implementation rejected every notification whose
turn ID differed from the new turn. Read-only protocol inspection isolated that
replayed telemetry; a dedicated regression and a narrow exception for token
accounting fixed resume. Wrong item/completion/thread identities still fail.

Attempt 1 is preserved, including its provider-history recovery. It did not
retain a process census or explicit disposable-directory cleanup assertion;
those missing measurements are not backfilled. Attempt 2 records both:
all observed owned process groups had no live members after each guard exited,
and its disposable working directory was removed. Native deterministic tests
also exercise guard cancellation while app-server is waiting for output.

## Confinement remains incomplete

The same native Codex binary ran separate, model-free named-permission canary
probes. A read profile allowed the synthetic in-root read and denied writes;
a write profile allowed in-root read/write. Both denied reads/writes in a
sibling synthetic directory with OS permission errors. These standalone
`codex sandbox` results are retained in each report. They do not qualify the
app-server launch, which still uses legacy sandbox parameters.

Ambient MCP startup notifications were observed during metadata inspection.
External tool/config isolation, app-server binding to the restricted profile,
full native provider cancellation/confinement receipts, a transport-bound
qualification schema, and an integrated reviewed Result remain outstanding.
Existing exec qualification cannot authorize the new app-server transport.

## Retained files and reproduction

Each attempt retains synthetic prompts, successful provider transcripts,
mechanical assessments, and a report with exact native executable BLAKE3/SHA256,
guard SHA256, script SHA256, requested selection, timing, and explicit blocked
admission status. Account details and raw stderr are excluded; stderr length
and digest remain. No API-key fallback was used. Timing includes provider
startup and shutdown and is not a model-compute-only benchmark.

The [exact attempt-2 script](attempt-2/probe-script.py) is retained separately
from later harness edits. A measured
[post-run identity comparison](attempt-2/post-run-identity-check.json) confirms
that its SHA256 and the native Codex and guard SHA256 values match those
recorded before the run; both primary transcript hashes match the report too.
This comparison occurred after the run, before the script's metric-name update;
it is a file identity check, not an in-process attestation. Future script runs
record the before/after comparison directly in their reports.

The [probe script](../../../scripts/agent_swarm_codex_probe.py) requires explicit
`--live`, `--model`, `--effort`, and `--acknowledge-moving-alias` arguments plus
exact native Codex, guard, and Swarm executable paths. Use a fresh `--output`
directory. It cannot emit or install production qualification records.

Validation: 8 protocol unit tests, 3 native transport fixtures, and 16 existing
execution regression tests pass. The provider remains unqualified regardless
of these fixture results or the successful synthetic turns.
