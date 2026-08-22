# IMO-53/57 Wave 9 live Agent Heist lane

Status: blocked, fail-closed (2026-08-21)

This lane inspected the current protocol, Python SDK, and SQLite daemon before
attempting implementation. No harness or server API was invented, and no
existing Rust, SDK, example, manifest, or release file was changed by this
lane.

## Exact blocker

The public daemon exposes Runner handshake and Activation offer/claim/renew/
release/complete messages, and the SDK exposes those same bounded operations.
However, the daemon scheduler only reclaims expired Activation leases. It does
not expose a public timer-fire, semantic-time advance, plan-proposal, or other
operation that creates the Agent Heist Activation transitions. The server
therefore cannot be driven from a fresh room to a complete deterministic Heist
using only the supplied public APIs.

The existing live Counter story confirms the daemon is reachable and ready,
but explicitly remains a Counter member-path proof rather than a complete
Agent Heist execution. The server-side Runner capability route test proves
Runner authority and an empty offer poll, not a real Heist invocation.

## Evidence inspected

- `cargo test --locked --workspace`: passed.
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `uv run --project sdk/python --locked pytest -q`: passed, including the
  Runner client tests.
- `PYTHONPATH=sdk/python/src uv run --project sdk/python --locked python
  examples/heist/wave6-live/run_live_story.py`: passed with real daemon HTTP /
  WebSocket traffic, `/readyz` 200 before and after restart, and a successful
  Counter action; result status was honestly `partial`.
- The server Runner-capability route test passed and verified Runner hello plus
  an empty authorized offer poll.

## Honest conclusion

No complete live Agent Heist result is claimed. A harness that pretended to
progress Heist state without a public transition-producing operation would
violate the protocol boundary and produce misleading evidence. The next
implementation prerequisite is a deliberately specified public server
operation (or a pre-seeded eligible Invocation fixture) that lets an external
orchestrator obtain a real Activation; once present, the existing SDK Runner
client can poll, claim, renew/release, and complete it.

No Linear issue was marked Done.
