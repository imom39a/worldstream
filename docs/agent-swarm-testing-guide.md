# Try a local Agent Swarm coding task

This guide runs the repository's local, subscription-backed Agent Swarm against
one small single-file task. Start with the no-model configuration check, then
build and run local tests, and use the managed smoke test if you want one before
an optional live run. The example is in
[`examples/agent-swarm/log-summary`](../examples/agent-swarm/log-summary/).

The live path starts local WorldStream services and may invoke the selected
provider. It consumes subscription allowance and writes run state. The runner's
`--workspace` is the repository checkout; its task files and state live below a
separate temporary `--root`. Never use an important directory as the state
root.

## 1. Copy and validate a problem

Start at the repository root:

```sh
cd /Users/vinothshanmugam/code/agent-streamer
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-managed.py \
  --validate-problem "$PWD/examples/agent-swarm/log-summary/problem.json"
```

The output must report `validated_no_model_or_managed_services`. The config
argument must be an absolute path with no symlinked path components. This
command needs no build, Room, workspace, or provider. To adapt the example,
copy its directory and edit the copy:

```sh
mkdir -p .scratch
cp -R examples/agent-swarm/log-summary .scratch/my-swarm-problem
```

Edit the goal, constraints, starter, and checker in that copy, then validate it:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-managed.py \
  --validate-problem "$PWD/.scratch/my-swarm-problem/problem.json"
```

Keep a new run's report and logs outside its state root.

## 2. Build and run fast checks

From the repository root, build the managed Agent Swarm, Server, and Studio
Supervisor binaries:

```sh
cargo build --locked -p worldstream-agent-swarm \
  --features managed-local-runtime --bins \
  -p worldstream-server -p worldstream-studio-supervisor
```

Run the Python tests that cover the live-run interface and recovery evidence:

```sh
uv run --project sdk/python --python 3.14.7 python -m pytest \
  tests/test_agent_swarm_live.py \
  tests/test_agent_swarm_problem.py \
  tests/test_agent_swarm_recovery.py \
  tests/test_agent_swarm_recovery_evidence.py
```

These controlled tests do not call a live model. For a managed protocol smoke
test, create a fresh temporary parent and keep its logs and report outside the
state root:

```sh
RUN_PARENT="$(mktemp -d /tmp/agent-swarm-smoke.XXXXXX)"
mkdir "$RUN_PARENT/state"
printf 'Run files: %s\n' "$RUN_PARENT"
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-managed.py \
  --workspace "$PWD" --root "$RUN_PARENT/state" \
  --pack "$PWD/packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack" \
  --autonomy-report "$RUN_PARENT/autonomy-report.json" \
  >"$RUN_PARENT/stdout.log" 2>"$RUN_PARENT/stderr.log"
```

The runner requires the repository checkout as `--workspace` and an existing
empty `--root`; the command above creates the latter. The runner creates its
own disposable work directory below that root. Keep the report and redirected
logs outside the root. Save the printed parent path: a second terminal will not
inherit the `RUN_PARENT` shell variable. A cleanup marker named
`managed-processes-stopped` in the state root means the runner proved cleanup.
If the command fails or that marker is absent, preserve the directory and
inspect its stderr and state journal; do not treat a missing report as success.

## 3. Problem format

The example config is a JSON object with exactly these fields:

```json
{
  "goal": "A concise, single-line task description.",
  "constraints": ["One concise, single-line constraint."],
  "acceptance_criterion": "What the configured checker establishes.",
  "target": "module.py",
  "initial_source_file": "module.py",
  "checker_file": "checker.py",
  "require_parallel": false
}
```

The source and checker paths are relative to the config file, must name regular
UTF-8 files within that directory, and may not traverse symlinks or `..` paths.
The config path itself must be absolute and have no symlinked path components.
The target is one basename ending in `.py`, `.txt`, `.md`, or `.json`. The
starter is at most 8 KiB; the trusted checker is at most 4 KiB and uses only
the Python standard library. It runs as
`python -B -c` with its current directory set to the candidate root, an empty
explicit environment, and the local Seatbelt sandbox; it passes only on exit
status 0.
Write the checker yourself and review it: the evaluator treats it as trusted
acceptance policy. Do not put secrets, credentials, shell substitutions, or
unsupported control characters in setup text. The validator rejects unsafe
setup strings instead of trying to filter around them.

`require_parallel` defaults to `false`. Setting it to `true` requires
Contributions from at least two authors and an observed overlap of at least
two native Invocations. The LLM still chooses the work plan. Inspect the
Contributions to judge whether that overlap was useful; overlap alone does
not establish a speed improvement. Leave it false when serial work is acceptable.

Validation checks config shape, file bounds and setup-safe text; it does not
prove the task will be solvable or that the checker is correct.

The example asks the Swarm to implement `summarize_log(text)` in
`log_summary.py`. Its checker exercises counts, blank lines, malformed input,
and unknown levels. The starter intentionally fails those checks. You can copy
the example directory and edit the goal, constraints, starter, and checker,
keeping each file within the limits above. This is a single-file goal, not an
arbitrary repository-project runner; a text, Markdown, or JSON target still
uses the configured trusted Python checker.

## 4. Run a live custom problem

Use a new empty state root for every run; the runner creates a disposable task
working area beneath it. This command requires macOS Seatbelt, the native
Codex executable, a signed-in subscription, and an admitted model/effort pair.
It starts the live run; it is not part of validation or the controlled smoke
test.

```sh
RUN_PARENT="$(mktemp -d /tmp/agent-swarm-log-summary.XXXXXX)"
mkdir "$RUN_PARENT/state"
SWARM_CODEX_BIN="/Users/vinothshanmugam/.nvm/versions/node/v25.9.0/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex"
printf 'Run files: %s\n' "$RUN_PARENT"
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-managed.py \
  --workspace "$PWD" \
  --root "$RUN_PARENT/state" \
  --pack "$PWD/packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack" \
  --live-problem "$PWD/.scratch/my-swarm-problem/problem.json" \
  --live-codex-report "$RUN_PARENT/report.json" \
  --codex-path "$SWARM_CODEX_BIN" \
  --live-model gpt-5.6-sol --live-effort medium \
  --acknowledge-live-moving-alias \
  >"$RUN_PARENT/stdout.log" 2>"$RUN_PARENT/stderr.log"
```

The example copies the task config to `.scratch/my-swarm-problem/problem.json`;
if you are using the checked-in example directly, replace that argument with
`"$PWD/examples/agent-swarm/log-summary/problem.json"`. The path in
`SWARM_CODEX_BIN` is the native executable admitted in the measured
runs. If Codex is installed elsewhere, set the variable to that installation's
native executable. Do not substitute a Node launcher or a different binary
without running native admission. The selected model and effort must pass
admission; a newer model choice is not qualified by this guide. The measured
execution configuration is `gpt-5.6-sol` at `medium`, not the GPT-6 Sol and
Luna development agents. The moving-alias acknowledgement is explicit because
provider aliases may change.

The `--workspace` argument is the repository checkout (the runner needs its
build and development files). The disposable task working directory is created
under `--root`; that root must be new and disposable. For a repeat trial,
create a new `RUN_PARENT`, state root, and report path. Do not reopen or reuse a
root with an uncertain effect.

The live custom mode uses the bounded managed lifecycle and autonomous
delivery path. The model chooses work and proposals; the host constrains
available Actions, policy-authorized checks, and the final writeback. Workers
do not receive arbitrary repository tools or Host credentials. The checked
artifact is one file in the disposable working area; the configured Python
checker can test `.py`, `.txt`, `.md`, or `.json` targets. A fresh independent
roster reviewer must approve it after the configured checker passes; then the
runner applies writeback and accepts a Result.

The current run is bounded to at most 4 Work Items, 40 model Invocations, 3
Candidate versions, and 2 concurrent native workers. The checker has a 20-second
timeout. The work loop has a 1,500-second elapsed budget; this is not a strict
end-to-end wall-clock deadline, since setup and cleanup add time.

Each `worker-run --once` command has a 90-second harness deadline. A timeout
can end a live run before work completes. Its cause is run-specific; inspect
the retained stderr and state evidence instead of assuming model slowness or a
particular backend failure.

Treat a run as successful only when the command exits 0, the report says the
trial passed, an accepted Result identifies the checked Candidate and current
passing Check/review evidence, writeback is reported, and
`managed-processes-stopped` exists. The report is written only on success. On
failure, use stderr, the retained Room/state journal, and the cleanup marker;
do not infer success from a partial artifact or model-authored claim. Preserve
the root until you have reviewed the evidence. Repeating an experiment means a
new empty root and new report path, not reopening an uncertain run.

In a second terminal, replace the path below with the `Run files:` path printed
before launch. Shell variables do not carry between terminals:

```sh
cd /Users/vinothshanmugam/code/agent-streamer
RUN_PARENT="/tmp/agent-swarm-log-summary.replace-with-printed-path"
tail -f "$RUN_PARENT/stderr.log"
```

Press Ctrl+C in this second terminal to stop following the log; it does not
stop the live run in the first terminal. Then inspect the report:

```sh
cd /Users/vinothshanmugam/code/agent-streamer
RUN_PARENT="/tmp/agent-swarm-log-summary.replace-with-printed-path"
uv run --project sdk/python --python 3.14.7 python -m json.tool "$RUN_PARENT/report.json"
```

The report command succeeds only after a successful run has written the report.
Otherwise inspect stderr and the retained state root; do not treat the missing
report as an accepted outcome.

In the report, `delivered_path` names your output file, `candidate_versions`
contains check output and reviews, and `accepted_results` records acceptance.
Compare `elapsed_seconds` and `max_observed_native_overlap` across fresh runs
of the same problem. Keep failed runs in your comparison and change one goal,
constraint, or checker assumption at a time so you can explain any improvement.

## 5. Optional recovery experiment

The repository also has a frozen baseline-recovery wrapper and predeclared
ordering requirement in
[`docs/evidence/agent-swarm-autonomous-recovery/TEST-DESIGN.md`](evidence/agent-swarm-autonomous-recovery/TEST-DESIGN.md).
It is an evaluation harness, not a general custom-problem mode. In particular,
it requires the defective baseline to be reproduced and actually fail its
checker before the model starts repair work. Keep that criterion unchanged;
the recent recovery attempts did not demonstrate recovery. Use a fresh root and
read the evidence README before considering another run.

For the optional recovery experiment, use a unique lowercase label
so the wrapper preserves prior attempts:

```sh
uv run --project sdk/python --python 3.14.7 python \
  .scratch/swarm-autonomous-recovery/run_trial.py attempt-5
```

This wrapper uses a fixed recovery task and records evidence under
`.scratch/swarm-autonomous-recovery/validation/`. Each `worker-run --once`
command has an unchanged 90-second harness deadline, which can stop a run
before an Invocation completes. The cause of a timeout must be established
from that attempt's retained evidence. Preserve every failed attempt; a retry
is a new experiment, not a continuation that replaces the earlier outcome.
The latest stopped recovery run, [attempt 4b](evidence/agent-swarm-autonomous-recovery/attempt-4b/summary.json),
hit that command deadline before producing a Candidate or Check; recovery
remains unresolved.

## Interpreting the evaluator

The local runner coordinates one Swarm Room and uses its selected subscription
provider. Work Contributions, an integrated Candidate, checker results,
independent review, writeback, and the accepted Result are distinct evidence.
A Contribution or a passing checker alone is not acceptance. The host enforces
schema and Action authority; the model decides its plan. The custom live path
has not been established as a successful end-to-end flow by this guide.

The judge is an independent roster Participant, not JEV. JEV is an optional
application-layer advisor and does not hold Participant, delivery, or
acceptance authority. Development subagents and the live Swarm are separate;
one run does not establish cross-run model learning or a general success rate.
