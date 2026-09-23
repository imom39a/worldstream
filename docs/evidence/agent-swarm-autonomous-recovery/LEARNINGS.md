# Recovery experiment learnings

## Live outcome and next development decision

Attempt 3 did not qualify as autonomous recovery. It authored the repair before
reproducing the failure, then stopped on an uncertain submission of the exact
baseline. The authoritative Room ends at sequence 10 with two Contributions
and no Candidate, Check, Review or Result. The 19 model Invocations included
nine planning turns and six proposal/claim turns. Two authoring turns overlapped
for 69.862 seconds; this establishes concurrency, not a speed advantage.

The next production increment should improve model feedback and planning before
adding broader autonomy:

1. Validate the compiled Action payload against the exact offered schema before
   submission and return bounded, field-level errors to the model. The empty
   `contribution_refs` list violates the Pack's minimum of one. Do not manufacture
   provenance or silently add a reference to make a proposal pass.
2. Preserve the distinction between a proven rejection and an unknown Action
   outcome. The current generic uncertainty is not proof of a returned schema
   rejection. Keep uncertain effects fenced; use exact Action identity and
   authoritative receipts when reconciling them.
3. Have the planner evaluate temporal prerequisites from the goal and Work Item
   before dispatch. Here, the repair Work Item itself said to wait for baseline
   failure evidence, yet the planner dispatched repair in parallel with tests.
   Keep this reasoning with the model rather than hard-coding CSV-specific phases.
4. Supply existing resource content through an explicit, bounded, digest-verified
   input mechanism. The current no-tools workers receive resource metadata, so
   the harness had to encode source in constraints. Planning must also account
   for the Pack's requirement that a Candidate cite an actual Contribution.

JEV has a concrete future shadow-mode evaluation case: can an advisory
assessment flag the premature repair dispatch in this retained history? Such a
comparison would follow [ADR 0043](../../adr/0043-use-jev-as-an-application-layer-swarm-advisor.md)
and would not grant JEV a review vote, work ownership or acceptance authority.
No JEV call or automatic policy change was made here.

## Setup must be checked through the actual controller

The first two attempts stopped before autonomous planning. Neither is evidence
of a Swarm reasoning failure or successful recovery.

Attempt 1 embedded the starter in a 7,045-byte goal. The application's domain
type accepts goals up to 16 KiB, but the managed backend and Pack cap them at
4 KiB. Attempt 2 used a shorter goal and bounded source chunks, but the generic
Room setup parser rejects literal control characters, including newlines, in
configuration strings. Both failures surfaced only as `backend data is invalid`.

The corrected input uses a single-line goal and numbered JSON string literals.
Decoding and concatenating those literals reproduces the same 4,826-byte
defective source. The fixed checker, criterion, source defect and execution
policy are unchanged. A setup-only preflight through the real controller and
installed Pack passed before attempt 3; its Controller and Runtime were stopped
and their leases released afterward.

These failures expose two practical improvements: validate the full request
before provider admission, and preserve field-level setup diagnostics at the
Swarm boundary. Tests that check only the application's input bounds miss the
stricter downstream contract.

## Evidence tests need actual protocol examples

Independent review caught assumptions that a synthetic positive fixture had
missed. Native turn output uses `session_id`, review targets identify Candidate
versions explicitly, and `submit_candidate` does not carry a version number.
The Pack assigns the version. The verifier therefore joins the selected
integration instruction's `next_version`, accepted Action, exact artifact,
Room Candidate and retained native response. A nested Contribution version is
not a Candidate version.

The focused suite has 24 passing tests. A separate compatibility replay parsed
the real Candidate and review outputs from the previous successful delivery
run. That earlier run is rejected as recovery evidence because it contains no
failed baseline followed by a repaired Candidate.

## Scope of learning

The development agents made the setup and verifier corrections. That is not
evidence of cross-run self-improvement by the Swarm. The measured experiment
asks whether the production planner can continue from a real failed check to
a revised Candidate, fresh review and delivery without evaluator steering.
It uses an explicitly instructed, seeded defect. Even a successful run cannot
establish spontaneous bug discovery, general reliability, or a causal account
of the model's internal reasoning.
