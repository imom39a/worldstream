# IMO-57 Runner SDK Wave 9

## Scope

This lane changed only the Python SDK, the dependency-light Heist Runner
harness, and this evidence note. It did not change Rust crates, compatibility
manifests, release scripts, or server code.

The SDK now exposes `Client.open_runner(...)` and `Runner` operations for:

- Runner handshake with `client.hello` in `runner` mode followed by
  `runner.hello` / `runner.ready`;
- offer polling, claim, renew, release, and complete;
- strict bounded request/response schemas, including Activation context,
  delivery, result codes, and lease bounds;
- explicit request IDs and correlated replies, with `LostRunnerReply` retaining
  the exact request identity for retry/idempotency recovery;
- protocol error redaction that prevents bearer-shaped values from appearing in
  exception text or details.

The integration endpoint is intentionally distinct from the participant Room
endpoint: `Client.ws_url()` remains `/v1/stream`, while
`Client.runner_ws_url()` and `Runner.connect()` use `/v1/runner/stream`.

## Evidence

Commands run from the repository root:

```text
uv run --project sdk/python --locked pytest -q sdk/python/tests/test_runner_client.py
# 5 passed

uv run --project sdk/python --locked pytest -q sdk/python/tests
# 33 passed

uv run --project sdk/python --locked ruff check sdk/python/src sdk/python/tests examples/heist
# All checks passed!

uv run --project sdk/python --locked ruff format --check sdk/python/src sdk/python/tests examples/heist
# 13 files already formatted

uv run --project sdk/python python examples/heist/run_runner.py --self-test
# deterministic JSON; fixture_only=true, live_evidence=false,
# stream_path=/v1/runner/stream

uv run --project sdk/python python examples/heist/run_runner.py
# exit 2; status=blocked,
# reason=base_url_bearer_and_runner_id_are_required
```

Parent post-review corrected the harness default supported pack from the stale
`worldstream.heist` spelling to the registered `worldstream.agent-heist`
identifier. The SDK suite (33 tests), Heist examples (11 tests), Ruff, diff
hygiene, and Python compilation were rerun afterward.

The tests use an in-memory fake WebSocket only. They prove wire shape,
dedicated URL selection, strict validation, request correlation, redaction,
and exact retry identity. They do not prove a live server, issued capability,
durable Activation, or real invocation context.

The harness is fail-closed when live prerequisites are absent. A live run still
requires a real Runner capability, Runner identity, and authorized Agent room
and Membership; it does not mint capabilities or fabricate offers. Therefore
this lane makes no live acceptance claim and does not mark IMO-57 or any other
Linear issue complete.
