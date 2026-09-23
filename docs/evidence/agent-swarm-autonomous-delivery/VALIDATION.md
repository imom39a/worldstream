# Implementation validation

Before successful attempt 2:

| Check | Result |
| --- | --- |
| Swarm Rust unit/integration/doc suite, `managed-local-runtime` | **190 passed**, 0 failed |
| Python evaluation tests | **25 passed** |
| Scoped Clippy, all targets, `--no-deps -- -D warnings` | Passed |
| Managed binaries | Built successfully |
| Live acceptance checks | **26 passed**, 0 failed |

The eight focused autonomy tests cover failed-check revision with persistent findings, changed destination/policy refusal, lost Action replies after reopen, stale scope/decisions, invalid operation feedback, real checker timeout, Stop, and fresh judge sessions.

Review caught a selection bug: a judge option used a fresh session during preflight, but selecting it could reconstruct a retained Resume profile. Selection now forces Fresh for autonomous CandidateReview and FindingResolution plans. The regression failed before the fix and passed afterward. [Red evidence](validation/judge-session-red-2.log), [complete passing suite](validation/rust-tests-developer-tools.log).

Earlier validation was blocked before native program startup. After the user enabled ChatGPT under macOS Developer Tools, startup probes and the full suite completed. The launcher process chain confirmed this installation runs under `/Applications/ChatGPT.app`. This establishes recovery after the setting change, not a controlled diagnosis of its cause. [Retry record](validation/developer-tools-retry.json).

The build used a task-local dependency-index wrapper to avoid slow shared-cache metadata scanning. Rust still validated dependency identities. The runtime and evaluation scripts were hashed before and after the measured run; no drift occurred. These are scoped Swarm checks, not a clean bill of health for the entire pre-existing dirty repository.

The evidence exporter verified retained native-output objects, Contributions, Candidate bytes, check evidence and output against content digests. It checked fresh judge-session separation, author/reviewer independence, writeback-before-acceptance ordering, cleanup, and the absence of evaluator delivery plans or post-setup Actions. Credentials and provider profiles were not exported.
