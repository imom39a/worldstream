# Activation completion live closure

Status: completed with parent verification (2026-08-21)

The public Heist run initially failed at `activation.complete` with a bounded
`storage_unavailable`. A temporary, non-secret SQLite trace identified the
exact cause as `InvalidColumnType(5, "context_bytes", Blob)`: the completion
transaction selected the BLOB `activation_intents.context_bytes` column into
`Option<String>`. The result was incorrectly collapsed into the generic
storage error.

The SQLite completion reader now uses `Option<Vec<u8>>`. The temporary trace was
removed completely. A focused regression exercises a leased Activation through
public authority/store completion, asserts `Completed`, then replays the exact
same completion request and asserts the durable idempotent result:

```text
cargo test --locked -p worldstream-sqlite registered_runner_controls_completion_then_durable_revocation_denies_immediately
```

The test passed, and both SQLite and server clippy passed with `-D warnings`.
The final disposable-daemon SDK run observed completed and idempotent first
Activation completion, reclaimed-generation completion, and stale old-generation
fencing without exposing Invocation Context bytes.
