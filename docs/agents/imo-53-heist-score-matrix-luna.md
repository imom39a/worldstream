# IMO-53 Heist score-matrix continuation

This bounded lane adds the missing explicit 3/5 partial-score witness to the
Heist acceptance matrix. It uses the existing `resolve_now` timer reducer
path: two commitments select the same plan, the plan has one deliberately
non-matching canonical field, and neither commitment contributes the required
resource. The reducer therefore records three true checks out of five as
`partial_failure`.

## Changed files

- `crates/worldstream-core/src/agent_heist_privacy_tests.rs`
- `docs/agents/imo-53-heist-score-matrix-luna.md`

No reducer semantics, manifests, release fields, server/UI/SDK/storage code,
or digests were changed.

## Verification

Commands run from the repository root:

```text
cargo test --locked -p worldstream-core agent_heist_privacy_tests -- --nocapture
cargo fmt --all -- --check
cargo clippy --locked -p worldstream-core --lib --tests -- -D warnings
git diff --check -- crates/worldstream-core/src/agent_heist_privacy_tests.rs docs/agents/imo-53-heist-score-matrix-luna.md
```

The focused test and the scoped formatting, Clippy, and diff checks passed.

## Remaining boundary

This proves the canonical in-process reducer/golden row only. It does not
close broader live Heist, browser, process-kill, cross-platform, release
manifest, or cross-backend acceptance requirements.
