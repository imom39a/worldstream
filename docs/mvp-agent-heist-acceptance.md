# Agent Heist CLI-first acceptance

Agent Heist is the visual and protocol conformance activity for the current
CLI-first MVP. The supported acceptance path starts with `worldstreamctl`. It
does not use the retired Studio web application.

Follow Sections 1–7 of [Getting started](getting-started.md). A complete run
must prove both of these independent paths against fresh local state:

1. `worldstreamctl client open` creates a one-use handoff for the approved
   standalone Agent Heist Activity Client. The browser receives only the
   selected Membership authority and installs an authorized Projection Reset.
2. Explicitly exported Membership and Runner credential files let the Python
   SDK example connect both required seats, receive agent work, submit only
   exact Action Offers, pass the automatic timed phases, and reach a non-empty
   Outcome.

The operator must use `worldstreamctl room inspect` as the source of readiness
and final Room evidence. A browser page load, a running process, or fixture
output is not proof that a Membership is synchronized or that a Room is
complete. The proof must preserve separate Host, Membership, and Runner
authority and must not publish credential values.

For the cutover gate, repeat the same flow after a retained-state restart and
confirm that canonical Replay reaches the exact committed lineage. Also run
the Negotiate public-participant check from the same guide. These checks prove
the supported CLI path; they do not claim a signed or externally qualified
release.

## Focused source checks

The deterministic SDK helpers have focused tests:

```sh
uv run --project sdk/python --python 3.14.7 pytest -q \
  tests/test_cli_activity_credentials.py \
  tests/test_cli_heist_activity.py \
  tests/test_cli_negotiate_activity.py
```

The standalone browser clients use the shared conformance lane:

```sh
pnpm activity-clients:check
pnpm activity-clients:build
node scripts/verify-activity-clients.mjs
```

These focused checks support the live run. They do not replace it.

## Retained internal harness

`examples/heist/mvp_live/run_mvp_acceptance.py` is retained as an internal
Controller, assignment-MCP, browser, recovery, and Replay regression harness
from the earlier IMO-78 delivery. It calls lower-level Controller HTTP
operations directly and therefore is not the supported operator workflow or
evidence that the CLI-first guide passed.

The harness and its frozen report fixtures contain `studio` names. Those are
retained compatibility identifiers for old schemas, report fields, command
flags, and state paths. They do not refer to a running Studio frontend. Do not
copy that harness as an onboarding example.
