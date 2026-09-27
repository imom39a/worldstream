# Agent Swarm Activity Pack

This portable Pack owns canonical Agent Swarm coordination facts while Core
continues to own admission, exact Room Heads, Memberships, authorization,
transcripts and Timer generations. Provider processes, stdout and artifact
bytes remain local application concerns; the Pack stores only bounded
attribution, version and artifact-reference metadata.

## Immutable revisions

- `0.1.0` is the retained creation-and-inspection revision. Its one-Action
  bundle remains isolated under `releases/0.1.0/`.
- `0.2.0` adds bounded work decomposition and claims, versioned Contributions,
  Candidate/Check/Review/Result acceptance, dependent work and blockers,
  reconciled and attributable ownership handoff, review corrections,
  Suggestions and Human Directions, scoped progress
  reviews, correction escalation, shared-resource conflict records, durable
  late output and explicit reopen.

Every mutable entity uses an exact expected revision in its Action payload.
That domain fence supplements rather than replaces Core's exact-Head Action
admission. One fixed progress Timer is scheduled after setup confirmation,
after a goal-scoped progress report and after reopen. A Timer fire records one
claimable goal review Work Item, while blockers and Directions can record one
concurrent obligation for each distinct Work scope. Duplicate triggers for the
same scope are suppressed. No Timer or obligation starts an Invocation.
Completion cancels the Timer and suppresses stale fires. Human Directions and
shared-resource conflicts require Human resolution; review findings require
the Human or a participant independent of both candidate author and reviewer.
Facts requiring Human attention remain explicit in participant projections
(`findings`, pending `suggestions`, escalated `problems`, unresolved
`resource_conflicts`, and reconciliation `blockers`) and the TUI highlights
them. They are not emitted as Core Attention Signals: those signals are Agent
activation requests and therefore may target only enabled Agent participants.
Ownership handoff creates a new authoritative attempt only after the prior
attempt is interrupted and its effects are reconciled with evidence. Pending
or uncertain writebacks and unresolved resource conflicts block handoff. The
receiver must already be an enabled Worker in the immutable roster, so the
receiver's provider/model/effort selection is retained rather than copied or
changed by a handoff. Worker Membership identities remain immutable. While the
Swarm is open, the Human coordinator may revise a roster member's requested
provider/model/effort for the member's next Invocation by binding that
member's exact `configuration_revision`. The Action never interrupts current
work, records moving-alias acknowledgement, and returns the selection to
`resolution_unreported`; the local application must still fail closed unless
that provider selection is available and qualified. Initial roster entries
must likewise provide `moving_alias_acknowledged`; an unpinned or moving model
name is rejected unless that acknowledgement is true. Initial configuration
does not supply a configuration revision—the Pack binds revision `1`.

## Required checks and native evidence bridge

The accepted criteria define the complete check policy without another caller
controlled list. Acceptance criterion array position `N` has the stable Check
identity `criterion-N` (`criterion-1`, `criterion-2`, and so on). A Candidate
may record one immutable outcome for each required identity at its exact
Candidate version and criteria revision. A failed outcome cannot be hidden by
appending a new revision for the same Candidate; renewed validation requires a
new Candidate version. `accept_result.check_refs` must list the complete
current set in criterion order, with each exact stored revision, and every
Check must have passed.

`record_check.evidence_refs` contains exactly one object with no additional
fields:

```json
{
  "artifact": {
    "artifact_id": "check-evidence-id",
    "digest": "blake3:<64 lowercase hexadecimal characters>",
    "local_path": "/approved/root/check-evidence.json",
    "media_type": "application/vnd.worldstream.agent-swarm-check-evidence+json;version=1"
  },
  "candidate": { "candidate_id": "candidate-id", "version": 1 },
  "candidate_artifact": {
    "artifact_id": "candidate-artifact-id",
    "digest": "blake3:<64 lowercase hexadecimal characters>",
    "local_path": "/approved/root/candidate.json",
    "media_type": "application/vnd.worldstream.agent-swarm-code-candidate+json;version=1"
  },
  "check_id": "criterion-1",
  "criterion": "The exact accepted criterion text.",
  "criteria_revision": 1,
  "resource_basis": [{ "resource_id": "input-id", "version": 1 }]
}
```

The candidate identity, authoritative Candidate artifact, exact criterion
text, Check identity, criteria revision and resource basis must equal the
Action and current Candidate exactly. Both artifact paths must be within the
approved working area. The native `CodeChangeService` bridge must serialize
its fully bound `CheckEvidence` as canonical JSON, retain those bytes at that
path, compute the stated BLAKE3 digest, and emit this reference. A process exit
string or narrative success claim is not Check evidence.

Each correction outcome binds the correction Work Item's exact completed
attempt and may be recorded only once for that attempt. A `useful` outcome
accepts only exact authoritative references already in Pack state, formatted
as `contribution:<id>:<version>`, `candidate:<id>:<version>`,
`check:<id>:<revision>`, `finding:<id>:<revision>`, or
`blocker:<id>:<revision>`, and validates that the referenced fact is relevant
to the correction and active Problem. Self-reports do not reset correction
history. An unresolved or escalated Problem owns its scope across cosmetic new
problem IDs, and unresolved blocking review Findings follow the explicit
integration Work/supersession lineage across Candidate IDs.

Public/operator projections expose only phase and bounded counts. Participant
projections expose current authorized coordination metadata and local artifact
references, never artifact bytes or supervised process output.

Run the local Pack gates with Node 24.18.1:

```sh
pnpm pack:check
pnpm pack:test
pnpm pack:build
pnpm pack:prove
```
