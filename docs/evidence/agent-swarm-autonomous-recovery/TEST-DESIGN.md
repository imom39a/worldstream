# Predeclared live recovery experiment

This design was recorded before launching the recovery trial. It extends the
[successful autonomous delivery run](../agent-swarm-autonomous-delivery/README.md)
with a real failure and repair requirement. A passing result must be established
by retained evidence; this document is not a claim that the run passed.

## Question and starting conditions

Can the production LLM planner reproduce a known failure, use actual checker
feedback to select a repair, obtain fresh independent review, and deliver the
repaired artifact without post-start evaluator steering?

The starting artifact is the prior delivered `csv_tool.py`, with exactly one
mutation applied before setup: remove the call to `_validate_quoting(text)` from
`parse_records`. The resulting 4,826-byte baseline has SHA-256
`51963d5fd5af026ef5c70a0d0889d04c9e69a0f41a8246fda4db560e8f7f206a`.
The unchanged fixed checker reports 25/26 passing cases; the failure is
`quote_in_unquoted_field`.

The evaluator supplies this existing defective source as task input and the
initial resource. The goal explicitly requires an unchanged first Candidate
and a real failed check before repair. This is an instructed bug-reproduction
workflow, not spontaneous failure discovery. The model is not asked to invent
a defect. The model must author the repair; the known-good prior source is not
supplied to the live agents.
An honest diagnostic review of the failed baseline is permitted; it cannot
substitute for a fresh independent review of the repaired Candidate.

Read-only workers receive resource metadata but not automatic resource-file
contents, so setup supplies the baseline text directly. Attempt 1 embedded it
verbatim in the goal and stopped during managed setup: that backend caps goal
text at 4 KiB, below the domain's 16 KiB bound. For attempt 2 the same exact
baseline bytes were supplied as numbered raw-text constraints, each within the
existing 2 KiB item bound. Attempt 2 still failed before Room creation because
the setup parser rejects control characters such as raw newlines in all text
fields. For attempt 3, goal text is flattened and each source part is a JSON
string literal; decoding and concatenation recover the identical defective
bytes, with the goal supplying the order and SHA-256 identity.
No new Candidate adoption authority or direct modification of coordinator
state is introduced for the experiment.

## Frozen conditions

- Keep the original 26 CSV, JSON and CLI checks and accepted criterion unchanged.
- Freeze the baseline, checker command, executable identities and evaluation
  scripts before the live attempt. Retain before/after identities.
- Use the previously qualified subscription-backed native `gpt-5.6-sol` at
  `medium` for this comparable live trial. Development work is delegated to the
  user-requested `gpt-6-sol` and `gpt-6-luna` subagents; those are separate from
  the measured Swarm roster.
- Preserve the bounded policy: at most four Work Items, 40 Invocations and three
  Candidate versions. Keep the reserved judge separate from authors.
- The evaluator may initialize, tick and observe the service and collect evidence.
  After setup it may not stage delivery plans, submit Room Actions, mutate sealed
  source, change checks, or tell the model how to repair a failed Candidate.

## Required evidence

1. Candidate version 1 matches the frozen defective baseline exactly and its
   actual guarded check fails. A fabricated failure or a passing first Candidate
   cannot qualify as recovery.
2. The planner receives retained failure evidence and chooses a later Candidate
   with different source bytes. Retained worker/native output must establish
   model authorship; a Room author label alone is insufficient.
3. The later Candidate passes fresh checks bound to its exact content, unchanged
   criteria and checker, and current resource basis. Earlier check evidence cannot
   satisfy it.
4. A fresh independent judge reviews the repaired version. Any blocking findings
   require explicit resolution. The failed baseline is never accepted as Result.
5. The planner selects authorized delivery. Applied writeback precedes accepted
   Result, with no evaluator post-setup Actions or delivery Invocations.
6. Confirm managed cleanup and unchanged measured identities. Preserve every
   attempt, including failures and development interventions.

If the model repairs before reproducing the baseline, fails to copy it exactly,
exhausts its budget or requires help, retain that outcome as an unsuccessful
experiment. Do not rewrite the requirement after seeing the result.

## Interpretation and deferred work

A success establishes bounded recovery in a seeded one-file workflow. It does
not establish cross-run learning, arbitrary repository autonomy, a reliability
rate, or a speed advantage. Native overlap and coordination time are measured
separately. Authored tests remain supplementary until an explicit acceptance
policy binds their exact Contribution versions and authorizes their executor.

The Pack only accepts `criterion-N` check identities derived from approved
criteria. A future authored-test gate should therefore begin with an explicitly
approved additional criterion or a separately versioned Pack contract; merely
adding an `agent-tests` label in the executor would not be sufficient.
