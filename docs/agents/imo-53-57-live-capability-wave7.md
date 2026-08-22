# Wave 7 live capability report

## Outcome

Implemented the smallest production path for a real local SQLite daemon story.

- Added `builtin_worldstream_registry()`, combining the independently validated Counter registry and the exact builtin Agent Heist revision. Counter v1/v2 behavior and Heist digest selection remain retained and selectable.
- Wired `worldstreamd` to the combined registry and SQLite gateway backend.
- Added `POST /v1/operator/member-capabilities`. It requires the authenticated `HostOperator` bearer, validates the exact room/member/principal binding and typed scope/idempotency inputs, registers a `RoomMember` capability through `AuthorityV1::change`, returns the bearer once, and never stores or logs plaintext bearer material. Reusing the idempotency key with a different freshly generated bearer fails closed; existing bearer material is not echoed.
- Corrected the Python SDK’s handling of the valid empty-stream `retained_floor = frame_head + 1` and nullable observation cursor.
- No compatibility manifest, release-readiness file, Wave 6 evidence script, or Wave 6 report was changed by this lane.

## Verification

All commands below were run in a temporary copy with only its generated compatibility mirror, because the shared worktree already has pre-existing `compatibility.toml`/`compatibility.json` drift. The actual worktree manifests were not changed.

- `cargo test --locked -p worldstream-core registry::tests -- --nocapture` — **6 passed, 0 failed**.
- `cargo test --locked -p worldstream-server --lib` — **37 passed, 0 failed**.
- `cargo test --locked -p worldstream-server --lib operator_member_capability_route_proves_counter_live_path -- --nocapture` — **1 passed, 0 failed**.
- `cargo build --locked -p worldstream-server --bin worldstreamd` — **passed**.
- `cargo clippy --locked -p worldstream-core --all-targets --no-deps -- -D warnings` — **passed**.
- `cargo clippy --locked -p worldstream-server --all-targets --no-deps -- -D warnings` — **passed**, including the corrected registry documentation (`WorldStream` backticks and `# Errors`).
- `cargo fmt --all -- --check` — **passed**.
- `uv run --project sdk/python pytest -q sdk/python/tests/test_room_client.py` — **22 passed**.
- A temporary `worldstreamd` process plus an external raw protocol client completed health, Counter room creation, capability issuance, attach, sync acknowledgement, observation acknowledgement, and Counter `increment` through room sequence 1. Output was redacted and emitted no secret material.

The SDK now consumes attach/sync/ack correctly for the empty stream. An SDK-based live action attempt still timed out waiting for the action reply, while the raw external protocol client completed that action; this is retained as an SDK action-response compatibility gap rather than expanding the lane.

Dependency-inclusive strict server Clippy remains blocked by six pre-existing documentation lints in `crates/worldstream-sqlite/src/migration_contract.rs` (SQLite backticks and `# Errors`), outside the permitted production paths. Direct tests in the shared worktree remain blocked by the pre-existing compatibility mirror drift. Neither blocker was bypassed by changing manifests.

## Changed paths

Production/test paths changed for this lane:

- `crates/worldstream-core/src/activity_pack.rs`
- `crates/worldstream-core/src/lib.rs`
- `crates/worldstream-core/src/registry.rs`
- `crates/worldstream-server/src/bin/worldstreamd.rs`
- `crates/worldstream-server/src/lib.rs`
- `crates/worldstream-server/src/sqlite_backend.rs`
- `sdk/python/src/worldstream_sdk/client.py`
- `sdk/python/tests/test_room_client.py`
- `docs/agents/imo-53-57-live-capability-wave7.md`

## Residual scope

The daemon still reports `scheduler = not_configured`; this report claims only registry selection and the live Counter path, not a full six-phase Agent Heist execution story. No release evidence or issue completion claim is made.
