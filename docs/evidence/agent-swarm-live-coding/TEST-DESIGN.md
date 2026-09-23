# Live Swarm coding evaluation

This evaluation tests the real supervised Swarm application on WorldStream, using the user's selected subscription-backed `gpt-5.6-sol` model at `medium` effort. No controlled-provider implementation or reference source is supplied to live agents.

## Goal and success conditions

Deliver a standard-library Python `csv_tool.py` with CSV parsing, formatting, JSON conversion and a stdin/stdout CLI. The fixed 26-case checker covers quoting, embedded newlines, CRLF, Unicode, empty strings, headers, row widths, unclosed quotes, record validation, round trips, JSON string preservation and valid/invalid CLI behavior.

Attempts 1–11 used 22 checks and never reached delivery. Before attempt 12, source inspection prompted four additional checks: bare-carriage-return preservation, quotes inside unquoted fields, junk after closing quotes, and extra record keys. Attempt 11's generated implementation passes all 26 under the evaluation's Python 3.14.7 runtime; this was an oracle coverage improvement, not a reproduced implementation defect. The 26 checks are frozen before each subsequent run. Models receive actual digest-verified CheckRunner output for review and revision; no implementation fix is supplied to them.

A passing evaluation requires all of the following in one Room:

1. The production LLM planner chooses the decomposition, members and legal next actions. The harness supplies no ordinary worker plans.
2. At least two distinct members contribute artifacts, with real native execution overlap. Artificial delays do not count.
3. A model-authored integration candidate cites the accepted contributions. Its exact bytes are sealed and checked in a read-only, network-disabled sandbox.
4. A reserved member, in a fresh provider session, independently reviews the exact candidate. It may request changes; no passing verdict is supplied by the harness.
5. The code-change service accepts the exact checked and reviewed revision. Its writeback service applies those bytes and the Room records the outcome while still open. WorldStream then accepts the Result and closes the Room.
6. Managed processes stop and disposable provider credentials are removed. Executable and harness identities remain unchanged during the run.

The test permits up to four ordinary Work Items, 32 adaptive Invocations, two concurrent native provider processes and three candidate revisions. A task-count ceiling is not a prescribed decomposition. Three consecutive failures stop the adaptive loop instead of silently spending indefinitely.

Attempt 13 selected serial implementation despite proposing independent work and was stopped by the evaluator. From attempt 14, the goal explicitly tells the model that two distinct agents must execute useful independent WorkAttempts concurrently for the trial to pass. This exposes the evaluator's existing concurrency condition; it does not supply task contents, assignments or an execution plan.

## Execution boundaries

WorldStream holds authoritative Room history and accepted state. The Swarm application handles LLM planning, process supervision, artifacts and integration. Models propose actions; the coordinator binds them to current authority and the Pack validates them. Event sourcing does not itself choose tasks or execute code.

Native providers have no tools in this evaluation. They author source through `artifact.inline_text`; the coordinator publishes the bytes at immutable content-addressed paths. This demonstrates parallel code and test authoring, not general tool-using repository agents.

From attempt 13, fixed checks run through the native process guard and macOS Seatbelt directly, under an explicit read-only, network-disabled profile. The check service captures the small system launcher and its exact profile/argv as evidence. Native canary tests separately verify outside-read, write and reachable-network denial. Model execution still uses the selected subscription-backed Codex executable.

Native admission is an explicit, expiring local development scope tied to the exact executable, process guard, model/effort, private named permission profile and working directory. Real fresh/resume, filesystem/network denial, cancellation and owner-loss receipts are required. This is not portable release qualification.

The production adaptive loop currently ends at Contributions. A visible evaluation driver stages integration and review through the same supervised coordinator and Room, then invokes the existing check, acceptance and writeback services. Successful delivery therefore does not establish a fully autonomous production delivery loop. The independent judge uses the same Codex model family; JEV is not the judge in this evaluation.

## Interpretation

Report actual task boundaries, accepted contributions, model decisions, execution intervals, candidate revisions, checks, review and writeback. Retain failed attempts. Parallel overlap is not a measured speedup against a serial baseline. Passing one small task is not evidence of general reliability, cross-run learning or self-training.

Implementation fixes made by the outer development agent are engineering improvements, not learning by the Swarm. Only model decisions that respond to retained evidence within the actual run count as autonomous adaptation.
