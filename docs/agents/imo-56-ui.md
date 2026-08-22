# IMO-56 UI reference

This note records the small first-party Agent Heist console implemented in `web/console`. It is a fixture-backed reference experience: it does not connect to a server, invent a protocol, submit actions, or require provider credentials.

## Surfaces

- **Public board** renders the public projection: phase/deadline, room sequence, role presence and activation markers, published clue claims, plans, endorsements, challenges, commitment count, aggregate checks/result, and a public milestone timeline.
- **Participant** renders exact typed Action Offers as disabled fixture controls. The panel makes Exact head, Resync required, Catching up, Faulted, and Quarantined states visible. The exact-head basis is metadata only; no action is sent or rebased.
- **Operator diagnostics** renders only bounded redacted membership, session/cursor, runner, Activation, timer, frame, and integrity/hash metadata. It deliberately omits participant projections, private clues, offers, commitment values, raw Core/Activity state, credentials, and model traces.
- **Replay** is explicitly read-only and uses the public projection fixture. Final reveal is a separate gate and is locked unless the fixture Activity Phase is `Complete` and current authorization is present.

The default fixture is in `Result`, which makes aggregate outcome data visible while keeping final reveal locked. Tests use a typed `Complete` fixture to verify the terminal gate without rendering private reveal content.

## Privacy/noninterference contract

The app mounts only the selected authorization view. Public DOM therefore does not contain participant Action Offer payloads or operator diagnostics. Fixture data is authored as public-safe, participant-offer, and bounded-operator types instead of accepting a raw state object. No component mutates canonical state or pretends that a disabled control is a successful action.

The result exposes aggregate checks and vote summary only. Individual commitment values and contributor identities remain withheld. Text is rendered through React text nodes; the fixture has no HTML execution path.

## Verification

From the repository root:

```sh
pnpm ui:lint
pnpm ui:test
pnpm ui:build
```

`web/console/src/App.test.tsx` covers public privacy/noninterference, bounded operator diagnostics, synchronization and integrity labels, replay read-only behavior, and final reveal availability only after `Complete`.

The IMO-56 issue could not be fetched from the configured GitHub account for `imom39/worldstream`; this implementation follows the supplied task description and the checked-in `docs/ui-architecture.md`, `CONTEXT.md`, and Agent Heist privacy requirements.
