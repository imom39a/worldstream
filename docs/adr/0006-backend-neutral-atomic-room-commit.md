# ADR 0006: Use One Backend-Neutral Atomic Room Commit

Status: Accepted, 2026-08-15

## Context

SQLite and PostgreSQL have different physical locking and failure surfaces, while WorldStream needs one observable Room order, stable retries after lost replies, and an all-or-none bundle spanning canonical history, serving state, timers, private delivery, Activation decisions, and results. Treating actor order, a conditional update, or a successful driver call as the commit point would make crash and unknown-COMMIT behavior backend-dependent.

## Decision

WorldStream prepares one immutable, versioned `PreparedRoomCommit` outside storage locks. It binds an Operation Identity and Canonical Request Hash to the indivisible observed Complete Head `(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)`, plus exact integrity generation, authority/capability, policy, and operation-specific input/timer witnesses. The prepared intent is either one `Advance` or one `DurableDisposition { Rejection | NoChange }`.

Every new write takes a transaction-scoped Room fence, rechecks identity and all witnesses, persists the complete prepared bundle, and uses durable database COMMIT as the sole linearization point. An Advance atomically includes Transition/hash lineage, the indivisible eight-field Complete Head, Core/Activity materializations, final Membership state, timer consumption/mutations, addressed Frames/frame heads, Activation decisions/intents/fences, and the applicable Semantic Receipt, including TimerFired. `NotApplicable` binds no identity, hash, or receipt. A disposition persists only its receipt and consumes no Room sequence. Pack calls, projections, network publication, telemetry, derived indexing, and paired snapshot writes never occur while locks are held; snapshots and delivery are postcommit.

The storage seam exposes one Room Commit resolution algebra on both backends: resolved Transition/Rejection/NoChange with `New | Existing`, `NotApplicable`, `Reprepare`, `Fenced`, `Conflict`, `RetryableKnownAbsent`, `Indeterminate`, or `Fault`. `Outcome` is reserved for an Activity Pack domain result. Unknown COMMIT is resolved on the authoritative primary with the original identity/hash before retry, reprepare, scan, acknowledgement, or publication. Only a proven-known-absent resolution permits bounded retry of the identical sealed plan.

## Considered options

- Relying on the in-memory Room actor alone was rejected because process failure and PostgreSQL concurrency outlive actor state.
- Retrying an ambiguous COMMIT as a new request was rejected because it can duplicate canonical work and change time-dependent input.
- Splitting receipts, timers, Frames, or Activation creation into later transactions was rejected because externally visible consequences could disagree with the accepted Transition.
- Writing snapshots inside the Room transaction was rejected because a replaceable cache would enlarge the correctness-critical lock and could roll back valid history.

## Consequences

SQLite may funnel Rooms through its dedicated controlled writer and transaction-start write reservation. PostgreSQL may commit different Rooms concurrently using a transaction-scoped Room-root row lock or equivalent guarded write under Read Committed with `synchronous_commit=on`; it has no global serialization or session-state correctness dependency. Both profiles must pass one failure-injection and transcript suite. Semantic Receipts or equivalent tombstones remain resolvable with Room lineage, including after archive. The cost is deliberate per-Room serialization and potentially duplicated prepared work when a witness changes, in exchange for portable crash semantics and no blind retry.
