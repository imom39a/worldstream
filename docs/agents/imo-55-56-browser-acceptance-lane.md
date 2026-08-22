# IMO-55/56 browser acceptance lane

## Scope

This lane is limited to the first-party console, its browser smoke harness, and
tests/docs. It does not claim a hosted service, release evidence, credentials,
or a live Heist mutation.

## Implemented

- Added explicit credential-free browser scenarios for public, participant,
  operator, historical Replay, terminal Complete, Loading, CatchingUp, Faulted,
  and Quarantined states.
- Expanded `browser-privacy-smoke.sh` to inspect every view and recovery
  boundary against a disposable Vite preview. It checks read-only Replay,
  disabled fixture Action submission, exact-head labels, terminal-only reveal,
  and forbidden private/credential markers.
- Public Projection and historical Replay now withhold stale bytes while
  Loading or CatchingUp. Faulted retains the last verified surface; Quarantined
  remains a separate fail-closed boundary.
- Live-session messages carrying a Room or Membership identity are rejected
  when they target a different configured session. The rejection is redacted.
- Retained observation ACK eligibility is buffered until `room.sync_acked`,
  then flushed through the highest received frame. Concurrent Action submission
  is disabled while a current Action is pending.

## Verification

From the repository root:

```
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm --dir web/console test
40 tests passed across 6 files

PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH pnpm --dir web/console run build
TypeScript and Vite production build passed

bash -n web/console/browser-privacy-smoke.sh
git diff --check -- web/console
passed

PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  START_PREVIEW=1 CONSOLE_PORT=4177 \
  web/console/browser-privacy-smoke.sh
passed: public, participant, operator, Replay, Loading, CatchingUp, Faulted,
Quarantined, and Complete routes; no forbidden private/credential markers

scripts/smoke-operator.sh
passed: disposable local daemon health/readiness/version/operator smoke
```

## External boundary

The local daemon exposes authenticated WebSocket/HTTP surfaces, but the native
browser WebSocket cannot set its required `Authorization` header and the
console has no safe credential bootstrap or pre-seeded Heist room/replay
endpoint. Therefore this lane does not claim browser-to-daemon authenticated
projection, Replay, or Action submission evidence. A header-capable,
operator-approved bridge and disposable seeded room are required for that
remaining acceptance.
