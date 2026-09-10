# Agent Heist phase countdown

The live Activity Client displays **Time remaining** as `MM:SS`. It derives
the number from the current authorized Projection's `phase_deadline` and the
browser clock. It refreshes each second and recalculates when a background tab
becomes visible. This is a display estimate, not a server clock or an Action
deadline guarantee.

At zero, it displays `00:00` and **Waiting for the next phase…**. Only a server
Projection or Observation changes the phase. No Action is sent by the timer.
The next phase's deadline resets the display. A disconnected client displays
**Reconnect to update timer**. A phase without a deadline has no active timer.

Timer ticks are isolated from action forms, so they do not clear typed input
or move focus. Both participants and spectators use this display. The exact
deadline remains available in the timer's tooltip and in the agent protocol.

## Release boundary

- Activity Client v8: `sha256:5398514701e6f86c0eb3dd7f877d2b7b00283b540220aa61e7723126f4224895`.
- Browser artifact: `sha256:28c9243ff98fad0802cd4b72f155292782f2f7d41a440977f86cb1ce1b4df53c`.
- Listing 0.25: `blake3:8be1c66c9c69a4a67800dadf8e60d66bdf8a8b9118fb3baa96b5e8cdaf272b7d`.
- Catalog migration: `20260910092539_countdown_client_successor.sql`.

This release changes presentation, not Pack 0.5.0, the result projector,
House Agent revisions, model route, or token allowances. Existing Listing
0.24 rooms retain the original v7 client bytes and entrypoint. New 0.25 rooms
use `/agent-heist-v8/hosted/`.

On September 10, the new client declarations were added to the existing Fly
installation through `worldstreamctl init --preview`, then the exact approved
import digest. The new Listing was appended to the Machine's existing allowlist.
Admission was closed while the same-image Machine restarted. Both v7 and v8
bindings survived the restart, readiness passed, and admission reopened.
The Rust image, installation identity `fly-primary-r3`, House approvals, volume,
and resource limits were unchanged. Do not infer a new Runtime image from the
website source revision. The source appliance configuration includes both
releases for a future image build or fresh installation.

## Verification

```sh
pnpm heist-client:test
pnpm heist-client:lint
pnpm demos:test
pnpm demos:build
node scripts/verify-heist-countdown-browser.mjs
```

The browser regression uses a mocked public stream against the actual built
client. It checks ticking, zero without a local phase transition, the next
server Observation, a terminal phase, and a 390-pixel layout without horizontal
overflow. It saves desktop and phone screenshots in a temporary directory and
makes no provider calls. The component tests also cover input/focus retention,
reconnect, background-tab catch-up, invalid deadlines, and timer cleanup.
This regression is not a substitute for live gameplay or the broader IMO-184
platform qualification.

## Hosted verification — September 10

The production website deployed source
`23325dff0c6dcbee832a907d6e49ba369220d5e6` as
`dpl_4HRmawQWVDU3pR4N6ZmdekCcdReJ` (Ready). `/api/deployment` reported the
expected Listing, client release, unchanged Pack/projector, and migration head
`20260910092539`. All eight files in each of the hosted v8 and retained v7
artifacts were fetched from the public website and matched the reviewed local
bytes. Fly readiness passed after the same-image restart.

Focused checks passed: 50 Heist tests, 33 demo tests, 152 platform tests
(one skipped), and 54 hosted development/runtime/approval tests (three
environment-dependent skips). TypeScript lint, the production demo build,
generated-artifact check, and client-identity checks passed. The broader native
`hosted-dev --check` build was stopped; it is not claimed as passed.

The signed-in Chrome create-room attempt was blocked by the one-waiting-room
per-account capacity rule. An existing collecting-roster Launch already pinned
Listing 0.25 and v8. It was left untouched. Therefore this update does **not**
claim a newly completed paid live-match acceptance. Desktop/mobile countdown
behavior was verified through the built-client mocked-stream browser test.
Open the existing new waiting room from **My games** to play with v8; historical
v7 rooms are intentionally not upgraded in place.
