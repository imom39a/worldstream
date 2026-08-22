# IMO-55 Wave 8: Action Reply Correlation

Status: complete for the bounded request/reply correlation fix.

## Change

`crates/worldstream-server/src/lib.rs` now correlates every dispatch reply and
error to the envelope's explicit `request_id` when supplied. Messages without a
`request_id` retain the existing `message_id` fallback. The server regression
test covers both paths.

No Python SDK source change was required: the existing `Room.act` regression
test already supplies a distinct response `message_id` and matches the
explicit `request_id`.

## Verification

- `cargo fmt --all -- --check` — pass.
- `cargo test --locked -p worldstream-server replies_correlate_to_explicit_request_id_and_fall_back_to_message_id` — 1 passed.
- `cargo test --locked -p worldstream-server` — 38 passed; 0 failed.
- `cargo clippy --locked -p worldstream-server --all-targets --all-features -- -D warnings` — pass.
- `uv run --project sdk/python --locked pytest -q` — 39 passed.
- `uv run --project sdk/python --locked ruff check sdk/python` — pass.
- `uv run --project sdk/python --locked ruff format --check sdk/python` — pass; 6 files already formatted.
- `git diff --check` — pass.

Compatibility manifests were not modified by this wave. The worktree remains
uncommitted as requested.

## Residual blockers

None for the action-reply correlation fix. Broader release and full live Heist
acceptance gaps remain outside this bounded Wave 8 change.
