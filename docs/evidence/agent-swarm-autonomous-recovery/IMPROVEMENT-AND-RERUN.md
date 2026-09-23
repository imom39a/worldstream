# Improvement and rerun

This note records the approved, generic changes after attempt 3. It separates
protocol changes and local verification from any new live result.

## Approved changes

An offered Action grants authority; it does not establish that its semantic
prerequisites are ready. Planning guidance now asks the model to identify
prerequisites in the accepted goal and Work Item, cite observed evidence in its
reason, and establish missing evidence or report a blocker before dispatch.
This is general planner guidance. It does not encode a CSV workflow or a fixed
task graph.

For a model-authored Action proposal, the production ManagedLocal backend
resolves the payload schema from the exact installed Pack revision and verifies
its canonical digest against the current offer. The coordinator validates the
proposal payload against that returned schema before submitting an Action.
Schema-invalid proposals are retained as `NotSubmitted` with bounded
field-level feedback; feedback does not echo payload values or undeclared
property names. This is a pre-submission validation path, distinct from a
Pack's authoritative typed rejection receipt and from `SubmissionUncertain`,
which remains an ambiguous external effect requiring reconciliation of the same
retained Action. Host-authored delivery Actions continue through their
controlled path and rely on Pack validation.

The rerun keeps the frozen task, acceptance criteria, fixed checker, baseline,
model selection and effort, invocation/work limits, and explicit recovery
policy. The baseline bytes remain SHA256
`51963d5fd5af026ef5c70a0d0889d04c9e69a0f41a8246fda4db560e8f7f206a` (4,826
bytes); the configured limit remains 40 Invocations and four Work Items. The
first Candidate must still reproduce those bytes exactly and fail the unchanged
fixed check before repair work begins. Do not dispatch or execute a repair Work
Attempt while waiting for that Candidate or its check, even if the repair
Contribution or Candidate would be submitted later. A baseline-preservation
Contribution may supply the required nonempty Candidate reference only if its
bytes are verified identical to that baseline; it is not a repair. Test-only
work may run before the check if it does not diagnose or repair the source.
Repair work remains selected only after the actual failed evidence, without
evaluator Actions after setup.

## Verification so far

I ran the focused Python recovery suites unchanged:

```text
uv run --project sdk/python --python 3.14.7 python -m pytest -q \
  tests/test_agent_swarm_recovery.py \
  tests/test_agent_swarm_recovery_evidence.py
18 passed
```

I also ran `tests/test_agent_swarm_live.py` once: 5 passed. Sol reported that
the full Agent Swarm package tests passed, including the corrected
matching-digest malformed-schema case:

```text
cargo test -p worldstream-agent-swarm --features managed-local-runtime
195 passed; 0 failed
```

The named regressions cover catalog digest drift and invalid shape, unavailable
schema lookup, retained feedback after reopen and its inclusion in the next
planning instruction, and empty Candidate references never reaching the
backend. The earlier malformed-schema fixture initially had a mismatched digest
and only exercised digest rejection; Sol corrected it and separately ran the
focused schema test (1 passed). Sol also reported `cargo fmt --all --check`,
managed-local Agent Swarm binary build, and scoped Clippy passing with no Swarm
warnings. Strict and ordinary Clippy failed on existing Core/Supervisor lints;
the scoped command allowed those three dependency lints only. The command list
and limitations are retained in
`.scratch/swarm-autonomous-recovery/validation/schema-validation-summary.md`.
These Rust results were reported by Sol, not independently run by me. They do
not establish live-model behavior.

The schema gate is intentionally scoped to model-authored proposals. This note
does not claim host delivery Actions pass through that local validator. Pack
validation remains authoritative on that controlled path. A schema lookup
failure or identity mismatch must fail closed before Action submission; an
unknown submission outcome remains fenced and is never auto-retried or
reclassified as invalid.

## Prior attempts and interpretation

Attempts 1 and 2 stopped during setup and remain preserved. Attempt 3 reached
the Room but was unsuccessful: Agent B submitted a repair Contribution before
the baseline was reproduced, contrary to the frozen ordering rule. The
subsequent baseline Candidate Action was marked `submission_uncertain`; the
retained Room database ends at sequence 10 and contains no committed Candidate,
Check, Result or writeback. The Action's empty `contribution_refs` also violates
the Pack schema, but no rejection receipt establishes that this was the
transport failure's actual cause. The correct retained classification remains
uncertain, not confirmed rejected.

Correction to the earlier diagnosis: attempt 3's first integration response
used `sha256:` in `expected_digest`. That field is supported; the decoder
requires a canonical `blake3:` digest. The local `provider_output_invalid`
classification was caused by the digest format, not by an unsupported field.
The narrative correction is also recorded in `README.md` and `DIAGNOSIS.md`;
the original attempt bytes remain unchanged.

## Attempt 4

The initial attempt-4 preflight used `/usr/bin/python3`, which lacked the
required `blake3` module. It wrote an identity snapshot but made no managed-root
or model Actions; that failed preflight is preserved. Attempt 4b was a fresh
managed run, not a resume or retry of attempt 3, launched with:

```text
uv run --python 3.14 --with blake3 python .scratch/swarm-autonomous-recovery/run_trial.py attempt-4b
```

It used the same
resolved interpreter path and SHA256 as attempt 3 and retained the frozen task
and limits.

Attempt 4b exited 1 after 560.892 seconds. Its retained metadata records
cleanup proven, all 25 measured identities unchanged, and the live declaration
matching the frozen declaration. Stderr reports that the challenge command
exceeded its 90-second deadline; the retained evidence does not establish why
that subprocess reached the deadline. The Room ends at sequence 3 with one
accepted, unclaimed integration Work Item proposing baseline reproduction and
checking. There are no Contributions, Candidates, Checks, Reviews, writebacks
or Results. One planner output was rejected locally for format; the next
planning decision and Work Item proposal were accepted. No repair work was
dispatched or executed, and the baseline Candidate/check gate was never
reached. Offline acceptance verification did not pass because no complete
report exists. This attempt demonstrates neither successful nor failed
failure-driven code repair, and no review or delivery claim is made.

Hashes establish artifact identity and retained provenance joins; they do not,
by themselves, prove that a model authored an artifact. This work makes no
cross-run self-learning claim.
