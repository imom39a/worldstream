# Implementation validation

Before live attempt 16:

| Check | Result |
| --- | --- |
| `worldstream-agent-swarm`, feature `managed-local-runtime`, unit/integration/doc tests | 181 passed, 0 failed |
| Live harness, native admission, managed cleanup, autonomy and challenge Python tests | 24 passed |
| Scoped Rust Clippy, all targets, `--no-deps -- -D warnings` | Passed |
| Ruff on the five evaluation scripts and two new test files | Passed |
| Managed-local-runtime application binaries | Built successfully |

The artifact-alias regression was first run against the old implementation and failed with `AmbiguousArtifact`. It passes after the explicit-descriptor fix, including rejection cases for absent identity, conflicting digests/media types and changed bytes. Existing path-only ambiguity rejection remains intact.

The native checker test executes real Seatbelt canaries. Candidate reading succeeds; reading an outside file, writing inside/outside the candidate, and connecting to a separately verified reachable local listener are denied. Check-output tests verify digest, length, path, symlink and presentation bounds.

These are scoped checks, not a claim that the entire dirty repository or portable release qualification passes. Full dependency Clippy has unrelated existing warnings; the reported command checks the changed Swarm crate with dependencies excluded from linting.

Local logs are retained under `.scratch/swarm-live-delivery/validation/`: `rust-tests-final-16-retry.log`, `python-final-16.log`, `python-managed-final-16.log`, `clippy-final-16.log`, `ruff-final-16.log` and `build-final-16.log`. A first test invocation was interrupted while macOS delayed executable startup; the complete retry above is the passing run.

The build used a task-local `rustc` wrapper that provides a hardlinked dependency index to avoid repeatedly scanning the very large shared debug cache. It did not replace compiled dependencies. Runtime executable identities are separately recorded and checked for drift during every live attempt.
