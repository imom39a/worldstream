# IMO-56 browser/privacy acceptance

Status: implemented and verified for the local first-party console; live Heist
execution remains fail-closed.

## Covered UI behavior

- Public and operator views render only their authorized/bounded projection
  fields. Private clues, Action Offer payloads, sealed commitments, raw Core or
  Activity state, provider credentials, and model traces are excluded from
  those DOM surfaces and from the allowlisted live-projection mapper.
- Participant Action Offers are withheld while Loading/CatchingUp and remain
  disabled unless the Session is Live, the Room is Healthy, the frame is Live,
  and the basis is the exact current Room Head.
- Submission checks the installed offer ID, action type, and schema digest. A
  stale result remains tied to its original Action ID and forces synchronization
  before a new ID can be created.
- Attach, retained Catch-up, Projection Reset, sync barrier, stale result,
  Loading, Faulted, and Quarantined states are visible. Quarantined public,
  participant, and Replay surfaces fail closed; bounded operator diagnostics
  remain available.
- Replay is read-only with a real fixture-backed historical sequence slider,
  explicit present and historical authorization labels, and no mutation path.
  Final reveal requires Complete, current authorization, and a Healthy Room.

## Verification

Run from the repository root:

```sh
pnpm ui:test
pnpm ui:lint
pnpm ui:build
CONSOLE_URL=http://127.0.0.1:4173/ sh web/console/browser-privacy-smoke.sh
git diff --check -- web/console
```

Observed on 2026-08-21:

- UI tests: 4 files, 30 passed, 0 failed.
- TypeScript lint: passed.
- Production Vite build: passed.
- Installed Google Chrome headless smoke against the production Vite preview:
  passed; public projection loaded and forbidden private/credential markers
  were absent from the browser DOM.
- Diff check: passed.
- The repository reports the existing engine warning because Node `25.9.0` is
  running while the console declares `24.18.1`.

## Fail-closed limitations

- The local fixture is not a live authorization source. It cannot prove a
  complete live Agent Heist, server-side authorization, or cross-session
  privacy boundary.
- The public daemon/API currently exposes no transition-producing operation (or
  pre-seeded eligible Invocation) that can drive the complete six-phase Heist;
  no fake offer, action, credential, or completion was created for acceptance.
- The installed Chrome binary was used for browser smoke. Playwright CLI was
  not used because its requested bundled browser executable was not installed;
  no browser download or dependency change was made.
- Browser smoke directly exercises the default public route. Participant,
  operator, replay, and quarantine DOM contracts are covered by focused React
  markup tests and pure live-session tests; no live server fixture was claimed.

No Linear status was changed.
