# ADR 0031: Verified checkpoint recovery with a bounded Transition tail

## Status

Accepted

## Context

Room recovery must construct the exact current executor, but replaying every
Transition from Genesis makes restart cost grow with retained history. Room
snapshots are disposable cache rows and may be deleted, stale, truncated, or
corrupt. Canonical Genesis and Transition rows remain the authority and must
remain available for forensic replay and backup verification.

## Decision

Recovery may use the newest paired snapshot whose sequence is within the
bounded tail window. This is a cache trust decision, not a claim that a
snapshot independently proves the semantic validity of every skipped
Transition. A checkpoint is trusted only after all of the following checks
succeed in one read transaction:

* its canonical Head is exact and agrees with the snapshot columns;
* its Core and Activity bytes are canonical, hash to that Head, and agree with
  the canonical Genesis/Transition row at the checkpoint sequence;
* the retained pack revision lock agrees with Genesis and the current Head;
* the complete Timer generation ledger is canonical and internally ordered;
* the immutable tail is read in sequence order from the checkpoint through the
  captured current Head.

The retained pack reconstructs an executor from the verified checkpoint state,
then replays only that tail. The final Head, pack lock, materializations,
integrity generation, and writer install fence are checked exactly as they are
for Genesis recovery. A missing, stale, malformed, or out-of-window checkpoint
is a cache miss and uses the existing full Genesis replay. Bounded recovery
therefore weakens synchronous semantic verification of the skipped prefix and
explicitly relies on the previously produced checkpoint for that prefix.
Diagnostic replay and backup verification remain the separate full-history
corruption detector and are invoked by their existing operator/backup
workflows; this ADR does not make them asynchronous serving work. Before the
bounded tail and install fence complete, the Room has no recovered serving
executor. After installation, it may serve under the healthy integrity fence
while a full forensic verification is run when required. Deployments that
require prefix verification before serving must select the full Genesis path,
or add an independently durable semantic accumulator in a future ADR.

Checkpoint rows contain no authority to publish data or mutate history. Timer,
membership, delivery, and frame projections remain operational witnesses and
are checked by the guarded install transaction; if the bounded path cannot
prove those witnesses, it falls back to full replay. Concurrent Head or
integrity changes abort installation and retry through the normal recovery
boundary.

The current SQLite schema stores Timer, observation-frame/consequence, and
membership-generation projections only at the live Head; it has no
checkpoint-keyed witness rows. Consequently a Room with Timers, observation
rows, or a noninitial Membership generation treats even a current-Head
checkpoint as a cache miss and uses Genesis replay. That rule prevents a
checkpoint from authenticating operational rows it does not contain. Supporting
bounded recovery for those ordinary active Rooms remains an open schema
follow-up: adding it requires cut-consistent witness rows and verification in
the same read transaction. The fallback is retained until that evidence
exists.

PostgreSQL remains the parity reference for the cold contract: its recovery
adapter currently captures and verifies the complete canonical history in one
transaction and does not yet persist an equivalent operational checkpoint
witness. It therefore keeps full prefix verification on every recovery until
the same witness schema and fencing rules are implemented there.

## Consequences

Eligible warm recovery reads one checkpoint plus at most 250 Transition rows
and runs the retained reducer only for that tail. Cold recovery retains the
stronger full replay path and its complete forensic diagnostics. Snapshots
remain replaceable caches and never become canonical history.
