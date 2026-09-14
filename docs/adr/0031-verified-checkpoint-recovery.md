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
* a canonical, hash-bound operational witness at the same checkpoint cut
  contains the complete Timer ledger, retained observation Frames and
  consequences, per-Membership generations and frame Heads, and Activation
  decisions;
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

Checkpoint rows contain no authority to publish data or mutate history. SQLite
migration 17 and PostgreSQL migration 18 persist the same canonical operational
witness at the checkpoint sequence. Each adapter reads that witness and the
bounded immutable tail in one database transaction. The guarded install then
compares the replay-derived operational facts with the current live
projections. A missing, stale, malformed, oversized, hash-invalid, or
internally invalid witness is a cache miss. A canonical witness that fails the
live operational comparison causes one full Genesis replay; the Room is
quarantined only if that authoritative path also proves corruption. Concurrent
Head or integrity changes abort failure recording and installation.

## Consequences

Eligible cold recovery reads one checkpoint plus at most 250 Transition rows
and runs the retained reducer only for that tail. Cold fallback retains the
stronger full replay path and its complete forensic diagnostics. Snapshots and
their operational witnesses remain replaceable caches and never become
canonical history.

The Transition and reducer work is bounded, while operational witness capture
and verification still scan the Room's currently retained Timer, Frame,
consequence, Membership, and Activation-decision rows. A witness larger than
16 MiB is not written, so that Room uses full replay. Compact accumulators or
partitioned operational witnesses require a separate design before claiming a
history-independent byte and query bound for Rooms with unbounded retained
operational state.
