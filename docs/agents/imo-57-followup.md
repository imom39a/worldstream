# IMO-57 follow-up: Heist/story-console parity

This follow-up adds local, dependency-light evidence for the deterministic
absent-Broker Agent Heist story and its first-party console fixture.

## Shared parity contract

[`examples/heist/parity_fixture.json`](../../examples/heist/parity_fixture.json)
is the checked-in canonical bridge between the Python story and the TypeScript
fixture. It records the story transcript digest
`sha256:6e69c62c2169e267736721c4f247d9558487394fd34e7572f1e949a391096ae7`,
the six public phases, the 15-transition replay head, the expected strict
two-of-three outcome, and the privacy boundary.

The Python self-test verifies the artifact against the generated transcript's
digest, final head, phase path, outcome, and replay count. The UI imports the
same JSON, validates those values at render time, and displays the shared
digest in its read-only Replay view. The fixture also asserts that public
projection excludes private clues, commitment values, participant offers, and
provider credentials; participant controls remain fixture-gated; operator
diagnostics remain redacted; and final reveal requires Complete.

## Fresh-checkout/offline evidence

`test_fresh_checkout.py` copies only the story runner, story module, and parity
artifact into a temporary clean checkout, runs the runner with the Python
standard library and a minimal environment, and verifies the 15-sequence,
read-only replay result. It does not require a broker, service, database,
network, browser, model provider, or installed third-party package.

## Verification

From the repository root:

```sh
python3 -m unittest discover -s examples/heist -p 'test_*.py'
python3 examples/heist/run_story.py --self-test
uv run --project sdk/python ruff format --check examples/heist
uv run --project sdk/python ruff check examples/heist
cd web/console
pnpm test
pnpm lint
pnpm build
```

Observed results: 7 Heist/story tests passed; story self-test passed; Ruff format
and lint passed; 10 UI tests passed; TypeScript lint passed; and the Vite
production build passed.

The parent also reviewed a separate SDK lane: 17 Python SDK tests pass with
strict canonical bearer/envelope/body validation, membership isolation,
sync-barrier handling, and retry identity preservation. Those checks remain
offline and do not claim a live browser/server transport.

## Remaining gaps

This remains local fixture evidence. It does not claim real process-kill,
power-loss, crash-durability, browser/server transport, SQLite/PostgreSQL,
SDK, broker, model-provider, or production UI integration behavior. The
canonical bridge is a fixture contract and is not a claim that the Python
SHA-256 evidence digest equals a Rust BLAKE3 pack digest.
