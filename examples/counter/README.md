# Counter

Counter is the smallest end-to-end kernel demonstration. Its acceptance runner
starts a disposable daemon, provisions a participant and a spectator, and drives
the public HTTP/WebSocket protocol through the Python SDK. It checks scoped
views, typed actions, and reconnect/restart behavior without a model provider.

From the repository root:

```sh
cargo build --locked -p worldstream-server --bins
uv sync --project sdk/python --locked
uv run --project sdk/python python examples/counter/run_live_acceptance.py --help
uv run --project sdk/python python examples/counter/run_live_acceptance.py
```

The runner uses temporary state and selects a free loopback port. The separate
`run_managed_acceptance.py` and deterministic loopback provider explore the
headless Controller/Runner path. Their checks remain examples, not release
qualification.
