# Agent Swarm native mixed-provider qualification

Ordinary CI uses controlled workers and must stay model-free. Readiness for a
real mixed-provider release is established separately on native macOS and
native Windows with normal signed-in subscription CLIs. Linux containers, WSL,
mock transcripts, or a deterministic adapter do not substitute for either
native run.

Repository tests establish deterministic contracts, Room/daemon separation,
exact Action handling, scheduler policy, process-guard containment, and package
structure. They do **not** establish that a real Codex, Claude Code, or Kiro
subscription session completed work. No complete external-provider or native
Windows qualification evidence is claimed here; those remain open release
evidence gaps until the documents below are collected and validated.

Each native target records one
`worldstream/agent-swarm-qualification-evidence/v2` JSON document. The complete
matrix covers `macos-arm64`, `macos-x86_64`, and `windows-x64`; a missing,
failed, or unsupported target/provider combination remains visible and keeps
readiness failed. A record binds the exact git revision, packaged-archive
SHA-256, `metadata/install.json` SHA-256 and identity, native target, application
version, and exact v0.2 Pack digest.

Each evidence document has a bounded content-addressed receipt manifest. Every
entry names a safe relative regular-file path, lowercase SHA-256, exact schema,
and receipt kind. Absolute paths, parent traversal, symlinks, oversized files,
hash drift, unknown schemas, unused receipts, and opaque references are
rejected. Provider and scheduler summaries are display data: they must exactly
match facts recomputed from typed receipts and never make a run pass by
themselves.

The record contains a 9–16 member roster and observed execution facts rather
than a single scenario checkbox. Those facts include the machine limit,
effective per-provider caps and observed concurrency, default and explicitly
equal priority values, Progress Review priority observation, configured budget
limits and their observed pause, and the review interval/slow-worker traffic
that produced due reviews. Each provider row has a `pass`, `fail`, or
`unsupported` verdict, reason, evidence references, CLI/executable identity,
at least two explicit model/effort selections when passing, and its native
assertions.

Passing providers also need one
`worldstream/agent-swarm-resource-confinement/v1` receipt for every qualified
model/effort selection. It binds the exact executable, invocation, working
root, resource policy, and allowed tools to the application process guard, and
retains three native probes: an in-root operation that succeeded, an
out-of-root escape that was denied, and an unapproved tool that was denied.
Missing or contradictory confinement evidence blocks the provider. This is a
qualification gate, not a claim that the repository already contains genuine
native confinement evidence for any external provider.

Every scenario row likewise carries a verdict, reason, bounded evidence
references, coordination-engine instance, goal, Swarm, Room, and exact
authoritative Head. Passing sourced-report and reviewed-code-change rows also
bind the accepted Result and Candidate, independent producer/reviewer members,
review identity, and artifact SHA-256. Boolean-only scenario maps are rejected.
This makes the two outcomes traceable to the retained coordination engine and
prevents a self-reviewed or unrelated artifact from satisfying the gate.

Run qualification from the verified v0.2 bundle, not a development binary.
Record the archive hash and `metadata/install.json` Pack identity, start the
packaged daemon with its exact sibling process guard, attach with
`--execution-state`, and exercise create/list/open/observe/act plus TUI
pause/stop/resume/detach. Confirm Room facts remain authoritative while daemon
status, budgets, priority, and provider caps remain operational-only. After a
daemon/application restart, confirm execution does not auto-resume.

Evidence should contain identifiers, hashes, versions, counts, verdicts, and
sanitized typed receipts; it must not contain provider credentials or full
private transcripts. Invocation receipts bind requested and reported settings,
configuration revision, disposition, Action identity, and times. Login
receipts retain a normal CLI probe with direct API-key environment removed.
Scheduler traces derive concurrency, caps, priority, budget pause, and timer
traffic. Scenario receipts bind the exact Head, criteria revision,
Action/Result lineage, and their source receipts. Failed and unsupported
combinations remain in the aggregate matrices and force aggregate status
`fail`. Do not edit them to `pass` to unblock a release.

After collecting one genuine document for every advertised native target,
assemble the release report and, only for a complete passing matrix, importable
per-target provider records:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/agent-swarm-qualification.py \
  --evidence evidence/agent-swarm-macos-arm64.json \
  --evidence evidence/agent-swarm-macos-x86_64.json \
  --evidence evidence/agent-swarm-windows-x64.json \
  --expected-revision EXACT_GIT_REVISION \
  --output evidence/agent-swarm-qualification-report.json \
  --provider-qualification-dir evidence/provider-qualifications
```

The aggregate is written even when its status is `fail`, and the command then
returns nonzero. Its complete target/provider and target/scenario matrices
retain `missing`, `fail`, and `unsupported` gaps for review. The optional
`--provider-qualification-dir` is created only for a passing full matrix. It is
a self-contained bundle: alongside one owner-readable v2 record per
target/provider, it copies the exact target evidence and every manifested
receipt without changing their bytes. Each record binds the evidence-file
hash, manifest hash, exact executable BLAKE3 digest, CLI version, tested
selections, and receipt-derived controls. Move the directory as a unit; a lone
record is intentionally not importable and is never portable approval for a
different binary or platform.

Install one generated record into a protected application-state directory:

```sh
worldstream-agent-swarm install-provider-qualification \
  --input evidence/provider-qualifications/macos-arm64-codex.json \
  --executable /absolute/path/to/codex \
  --output /absolute/private/state/provider-qualifications/macos-arm64-codex.json
```

The command opens the record's bundled evidence, rejects unsafe paths, re-hashes
the evidence and every receipt, and independently derives login, exact setting
reporting, selection isolation/revision, safe non-submission, process
containment/cancellation, and per-selection resource confinement. Only then
does it probe and canonicalize the executable and publish a new owner-only
installed binding; it does not overwrite an existing record. A hand-authored
boolean-only record, legacy v1 record, missing receipt, or edited receipt cannot
be installed. Then validate a requested selection against that bound
executable:

The installed binding retains the canonical path of the portable record and
re-verifies that record and its evidence graph on every load. Keep the
qualification bundle at that path; deleting, moving, or modifying it makes the
installed binding fail closed.

```sh
worldstream-agent-swarm qualify-provider \
  --qualification /absolute/private/state/provider-qualifications/macos-arm64-codex.json \
  --model EXACT_MODEL \
  --effort EXACT_EFFORT
```

`qualify-provider` re-probes the bound executable and fails closed unless its
provider, current executable bytes, version, native platform, model, effort,
and recorded assertions match. Coordinator commands consume these installed
bindings directly with repeated `--provider-qualification` arguments; they do
not accept caller-authored capability JSON. The command does not log in or
execute a model turn.

The assembler fails closed for missing advertised targets, absent
Codex/Claude/Kiro rows, a roster outside 9–16, insufficient distinct settings
for a passing provider, invalid capacity/concurrency or scheduling facts,
boolean-only scenarios, non-native or mismatched target identity, mismatched
package/install/Pack identity, revision skew, duplicate targets/runs,
incomplete exact-Head or Result lineage, self-review, failed scenarios, or
false provider assertions. Structural contradictions are rejected; genuine
failed/unsupported observations are retained in a failure report. It refuses
to overwrite an existing aggregate. A generated report therefore summarizes
supplied evidence; it is not itself a way to perform or simulate
qualification.

Provider discovery output alone is not qualification. Missing binaries,
unsupported versions, unresolved effective model/effort, login requirements,
or any attempted substitution must remain a blocker in evidence. Never place
credentials or private full transcripts in an evidence document.
The repository does not claim complete external-provider or native Windows
qualification. Local Codex probes are partial evidence, separately documented
in [adapter status](agent-swarm-codex-app-server.md); they do not satisfy the
release matrix or create installable qualification records.

## Current Codex and Claude compatibility status

The installed Codex and Claude CLIs can be discovered and prepared, but their
current non-interactive output contracts do not yet close this qualification
gate. Codex `exec --json` does not promise the effective model/effort event the
strict decoder requires. The new guarded Codex app-server driver verifies
provider-reported model and effort before starting a turn, supports fresh and
resumed threads, and fails closed on substitutions. It still needs genuine
native configuration-isolation and confinement evidence; protocol fixtures do
not qualify a provider. See [adapter status](agent-swarm-codex-app-server.md).
Claude's startup event reports its model,
while its effective effort is not a qualification-grade reported field.

Agent Swarm therefore keeps both providers blocked. Codex needs genuine native
qualification evidence; Claude needs a
provider-reported effective effort contract and genuine evidence. The current
adapters still isolate the controls that can be enforced: direct API
credentials are removed, Codex app-server settings are explicitly checked
(ambient configuration isolation remains unverified),
Claude setting sources and model/effort override environment are removed only
inside the owned child, provider-generated/resumed session IDs are checked,
and native delegation tools remain disabled. Normal subscription login status
alone is not model/effort, cancellation, or confinement evidence.

## Current Kiro compatibility status

Kiro remains deliberately unsupported under this qualification contract. Its
documented [headless mode](https://kiro.dev/docs/cli/headless/) requires
`KIRO_API_KEY`, which conflicts with the normal subscription-login-only rule in
ADR 0038. [ACP](https://kiro.dev/docs/cli/acp/) documents session model
selection and cancellation but does not provide the required invocation-scoped
effort selection plus effective model/effort receipt. The documented
[CLI commands](https://kiro.dev/docs/cli/reference/cli-commands/) expose effort
without an equivalent invocation-scoped model control, while custom-agent
[model configuration](https://kiro.dev/docs/custom-agents/configuration-reference/#model-field)
may fall back to a default model. Agent Swarm therefore reports the installed
CLI as blocked and cannot admit a Kiro Invocation or accept its output. It also
scrubs `KIRO_API_KEY`; introducing an API-key fallback is not an escape hatch.
