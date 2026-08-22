# IMO transition Luna report

Status: implemented and verified at the public seam (2026-08-21).

## Contract

`POST /v1/operator/rooms/{room_id}/timers/fire`

```json
{"timer_id":"<bounded id>","generation":1}
```

The server loads the exact durable Timer row by room, ID, and generation. It
does not accept or return the scheduled timestamp or payload. Only a due,
scheduled row may enter the commit path; a fired row resolves the same
identity/hash for an idempotent duplicate response.

## Implementation

- Core adds `AuthorityV1::authorize_timer_fired`, opaque
  `AuthorizedTimerFiredV1`, and
  `PreparedRoomCommitV1::for_authorized_timer_fired`. The grant binds the
  Room-root HostOperator scope and exact request hash; the sealer delegates to
  the existing Timer witness and Existing-Room commit/reprepare/indeterminate
  fences.
- SQLite adds exact Timer candidate loading and the production typed commit
  adapter. Recovery, writer, generation, receipt, and timer-row witnesses stay
  in the existing guarded path.
- Server adds the operator route, bounded protocol types, telemetry mapping,
  and safe transition/head response metadata. Raw Timer payloads are redacted.

## Focused evidence

- Core authority binding test: passed.
- Core Room Commit focused suite: 18 passed.
- SQLite exact Timer candidate lifecycle test: passed.
- Server bounded route/redaction test: passed.
- Workspace package check and `cargo fmt --all`: passed.

No live Heist witness is claimed in this report. The code now has the real
public transition seam, but a live fresh Agent Heist Room plus Runner was not
executed in this verification turn.
