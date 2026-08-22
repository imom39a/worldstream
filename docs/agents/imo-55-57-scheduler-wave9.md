# IMO-55/57 Scheduler, Runner, and Live Boundary Wave 9

## Parent-owned implementation

The parent reconciled the scheduler transport and Runner boundary after the
initial lane did not return a durable report. The server now owns a bounded
Activation scheduler runtime: it performs a startup tick before publishing
readiness, continues bounded lease-maintenance ticks, and joins the runtime on
operator-state drop. The daemon starts with `scheduler = "running"` and
`/readyz` is `200` only after storage, authority bootstrap, and scheduler
startup facts are established.

Runner control is separated from the participant Room stream:

- `GET /v1/stream` accepts participant/spectator/operator client modes;
- `GET /v1/runner/stream` accepts Runner mode only;
- `POST /v1/operator/runner-capabilities` is HostOperator-authenticated,
  validates the exact Agent Membership allowlist and scopes, installs the
  Agent principal, Runner registration, and Runner-control Capability through
  typed authority changes, and returns a one-time bearer with cache disabled;
- offer polling, claim, renew, release, and complete remain authorized against
  the Runner Capability's bounded `(Room, Membership)` allowlist.

The operator Runner route uses three distinct authority idempotency keys because
principal creation, Runner registration, and Capability registration are three
separate durable authority changes. A retry with a newly generated bearer and
the same Capability key fails closed rather than returning a bearer that is not
bound to the stored hash.

## Parent verification

Focused checks passed:

```text
cargo test --locked -p worldstream-server --lib       # 39 passed
cargo test --locked -p worldstream-server --bin worldstreamd  # 2 passed
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The new Rust route test creates an Agent Counter Membership, issues a Runner
Capability through the HTTP route, performs `runner.hello`, and authorizes an
empty offer poll through the backend. It asserts `no-store` and `no-cache`
headers and never prints the bearer.

The independent real-process probe then used a disposable daemon and the
actual Python SDK. It proved:

```text
readyz=200
Client.open_runner -> /v1/runner/stream
runner.ready=correlated
activation.offers=correlated, offer_count=0
secrets=not_emitted
```

The Counter live story was updated to assert `readyz=200` before and after
restart. It still reports `partial`: scheduler readiness and Runner transport
are live, but the story does not claim a complete Heist invocation.

## Luna SDK lane

Carson the 2nd implemented the disjoint Python SDK/example lane. The SDK now
provides `Client.open_runner`, dedicated Runner URL selection, strict bounded
Runner handshake and Activation operations, request correlation, lost-reply
retry identity, and secret-safe errors. `examples/heist/run_runner.py` is
fixture-only by default and fails closed when live prerequisites are absent.

Parent independently reran:

```text
uv run --project sdk/python --locked pytest -q sdk/python/tests  # 33 passed
uv run --project sdk/python --locked ruff check ...              # pass
uv run --project sdk/python --locked ruff format --check ...     # pass
uv run --project sdk/python --locked pytest -q examples/heist   # 11 passed
```

No live Heist acceptance is claimed by the SDK lane.

## Remaining acceptance boundary

The transport and authority seam are now real and tested, but the full
absent-Broker acceptance is still open. The current server has no public
operator/participant orchestration endpoint that can seed and drive the full
deterministic Heist activation sequence, and no live proof yet covers a real
offer followed by claim, invocation context, completion, restart recovery, UI
visibility, and replay of that same live Room. The offline Heist reducer and
privacy corpus remain the authoritative evidence for those unproven pieces.

No IMO-55, IMO-56, or IMO-57 status was changed to Done. No fabricated live
offer, invocation context, or release evidence was added.
