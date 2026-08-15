# ADR 0007: Use Typed Semantic Time and Fixed-Cutoff Timer Catch-Up

Status: Accepted, 2026-08-15

## Context

Wall clocks, database clocks, scheduler scans, queue timing, and process restart cannot provide a replayable total order. A universal Transition timestamp would also conflate distinct meanings: whether an Action met a deadline, when a Timer was semantically scheduled, and when administrative or external input was recorded. Timer/Action races and overdue recovery must behave identically on SQLite, PostgreSQL, restart, and Replay without distributed consensus.

## Decision

WorldStream has no universal Transition clock. An Action records HostClock `admitted_at` atomically with a successful bounded Room Admission Lane reservation; a Timer Generation has immutable `scheduled_for`, reused as TimerFired's semantic time; other Stimuli use their declared `recorded_at`. Detection, lag, dequeue, transaction, receipt, and commit times are operational only. Deadline windows are half-open, `open_at <= admitted_at < deadline`, so equality is late.

Participant Actions, existing-Room canonical administration, and newly due timers share one bounded per-Room lane with reserved host-stimulus capacity and no overtaking; Room creation has no existing Room or lane. Lane positions are provisional; durable database COMMIT alone orders races. WorldStream owns never-reused monotonic generations. Packs request only ScheduleNext, CancelCurrent, or RescheduleCurrent, and every new schedule must be strictly later than the causing Stimulus's typed Semantic Time. Timer firing has no `fired_at` and no separate claim; the exact immutable scheduled-generation witness is consumed by its Advance.

On verified load, a Room with overdue timers enters CatchingUp. The host captures one HostClock cutoff and drains every still-applicable generation due through that cutoff, one at a time in `(scheduled_for, timer_id, generation)` order, rereading after each result and including overdue cascades. Bounded slices may yield to other Rooms and runtime duties, but ordinary same-Room canonical commands do not interleave before the fixed cutoff drains. Every still-scheduled due generation remains an obligation until fired, canonically cancelled/rescheduled, archive-cancelled, or temporarily fenced.

## Considered options

- One generic logical timestamp and pack-visible `fired_at` were rejected because retries and restart would change semantic input.
- Client or database time was rejected because trust, precision, and backend behavior differ.
- Giving timers unconditional priority was rejected because it would overtake already admitted timely Actions; letting participant queues dominate was rejected because timers could starve.
- Dropping, coalescing, or processing overdue timers against a moving cutoff was rejected because resource budgets would change Room history.

## Consequences

The application needs an injectable normalized-UTC HostClock, a bounded fair Room lane, host-owned generation validation, deterministic fake-clock conformance, and an explicit `Loading -> CatchingUp -> Active` lifecycle. Clock rollback or an untrustworthy discontinuity fails new time-bearing work closed rather than reopening deadlines or rewriting history. Replay reads recorded typed fields in Room order and never starts HostClock or the scheduler.
