# Findings from live attempts

The failed attempts are part of the evaluation. They are retained in [attempts.json](attempts.json); none is counted as successful delivery. The outer development agent made the implementation changes below. That is engineering work, not autonomous learning by the Swarm.

## Native execution

Early attempts exposed incorrect executable selection among multiple installed Codex versions, an overly short debug-build startup deadline, a newline-invalid setup payload, and an environment allowlist that rejected the exact measured private Codex profile. These were fixed without permitting arbitrary provider environment variables or changing the normal release qualification gate.

A genuine macOS shutdown race was captured: `killpg` returned `EPERM` while the first `waitid` observation still reported no terminal status. Other captured turns showed the leader becoming observable as terminal immediately afterward. The guard previously rejected valid completed turns at that boundary. It now gives the already-gone error cases a bounded 240 ms observation grace, requires actual terminal status, and then reaps the group. It still rejects a live leader or observation error. A focused transition regression and native EOF/cancellation fixtures cover these boundaries. Temporary diagnostic logging has been removed from runtime code.

## LLM contracts and recovery

Real model output exposed cases the controlled provider did not exercise:

- A proposal batch selected several member alternatives that shared one Work ID.
- A task-proposal response prematurely attached implementation source.
- Planning steps included `dependency_ids: []` where the field must be absent.
- Worker responses omitted the required schema identifier.
- A planning response contained malformed JSON escaping.

The coordinator now retains rejected planning decisions with precise feedback, while keeping the existing invocation and consecutive-failure limits. Worker feedback identifies contract errors from retained output. Prompts state action-specific artifact rules, explicitly require the response envelope, and preserve the original action contract alongside the model-authored instruction.

Attempt 10 provides a real model correction: a rejected two-claim batch was followed by a single-claim decision that explicitly acknowledged the metadata rule. That attempt was later stopped by the evaluator after the planner chose serial implementation before claiming independent test work. It is not a parallelism success.

## Scheduling semantics

The production planner reassesses after every selected batch finishes. The model initially reasoned as if it could start one task and arrange the other while that task ran. The planning instructions now explain the barrier and the option to establish distinct claims before dispatching independent owned WorkAttempts together. This explains the execution interface; it does not hardcode task content, count or assignment.

The current barrier and separate model turns for proposal, claim and work incur overhead. The measurements are a capability test, not a throughput benchmark. Potential follow-ups are native structured-output constraints, safe reduction of repeated executable hashing during option preflight, and planning during already-running independent work. None of those is claimed as completed here.

## Parallel execution and malformed action boundary

Attempt 11 established distinct implementation and independent test-authoring tasks, claimed both, then dispatched their WorkAttempts together. Native turn timestamps show **85.989 seconds of overlap**. Both providers produced output, but neither output became a delivered Result in that attempt. This is evidence of useful concurrent authoring, not a completed end-to-end run.

The implementation author included a `digest` field in a `resource_basis` entry whose schema permits only `resource_id` and `version`. The Runtime returned `invalid_payload`; the managed adapter flattened that response into an uncertain submission. The coordinator correctly stopped further progress at the uncertainty boundary, but the evaluation harness kept polling instead of stopping. An explicit retry and SDK diagnostic reused the exact original Action identity and payload; neither altered or bypassed the Action. The evaluator then stopped the attempt and retained its journal and the other worker's unharvested completion.

The coordinator now rejects malformed resource-version references before submitting an Action and retains a precise explanation for the planner. The evaluation driver now stops promptly when explicit reconciliation is required. A general fix for the adapter's loss of definite protocol errors remains open: it must distinguish a confirmed rejection from genuinely unknown transport outcomes without replaying an uncertain mutation under a new identity.

## Check feedback and oracle coverage

Before attempt 12, the checker grew from 22 to 26 fixed cases to explicitly cover additional quoting, record-shape and carriage-return behavior. A suspected carriage-return bug in attempt 11's implementation did **not** reproduce under Python 3.14.7: that source passes all 26. No implementation defect is claimed from inspection alone.

The delivery driver now gives the integrator and independent judge bounded, digest-verified stdout and stderr from the exact CheckRunner invocation. Revisions can respond to actual failed cases. Focused tests reject corrupt bytes, path substitution and symlink substitution. The checker itself remains frozen throughout each live attempt.

Attempt 12 completed adaptive decomposition, parallel authoring, accepted both Contributions, and accepted an integration Candidate. It then stopped before checking because the check service captures its launcher executable as an artifact: the 220,538,240-byte Codex binary exceeded the existing 64 MiB bound. Before attempt 13, the evaluation driver switched the checker launcher to the 102,560-byte macOS `/usr/bin/sandbox-exec`, leaving model execution and the artifact limit unchanged. Its explicit Seatbelt profile allows read access to the candidate, Python runtime and required system files, permits directory-ancestor lookup, and denies writes and networking. Native tests verify denial of outside file reads, candidate/outside writes and a connection to an independently reachable localhost listener.

A diagnostic replay of attempt 12's exact candidate through the real guarded CheckRunner passed all 26 checks with the new launcher. This occurred after the live attempt had stopped, was not recorded as a Room check, and does not convert attempt 12 into a successful delivery. A fresh end-to-end attempt is required.

Attempt 13 again chose to execute implementation alone before claiming the independent test task. The evaluator stopped it; this is an observed serial decision and an operator-cancelled attempt, not proof that the model could never recover later. Before attempt 14, the goal was amended to explicitly expose the existing requirement for useful concurrent execution. Earlier successful overlap did not establish reliable scheduling across fresh runs. Model task selection remains unchanged in code; the added text explains the evaluation condition and the existing batch barrier.

Attempt 14 completed decomposition, concurrent authoring, integration, 26 passing fixed checks, independent passing review and Room Result acceptance. It did **not** deliver the file: the evaluation driver called `accept_result` before requesting writeback, closing the Room and withdrawing writeback offers. This was a new-driver ordering bug; the existing controlled challenge already used the correct sequence. Before attempt 15, delivery was moved before Room Result acceptance, after the separate code-change service's exact check/review gate has accepted the revision. No Pack authority rule or completion rule changed. A post-stop held-out CLI probe also confirmed preservation of quoted CRLF field content under the measured Python runtime; it did not reveal an additional source defect.

## Content aliases during independent review

Attempt 15 produced two accepted Contributions with 81.057 seconds of native overlap. Its integrator reused the implementation bytes exactly, so the Contribution and Candidate had different semantic artifact IDs at one immutable content-addressed path. The fixed checks passed, but preparing the judge's context failed: bounded capture resolved by pathname before checking the explicitly selected descriptor and treated the legitimate alias as ambiguous. The integration source was not required to change merely to obtain a different file path.

The fix permits explicit selection of an exact Room descriptor when all references to that path agree on digest and media type. Missing identities, conflicting digests/types, changed bytes and unsafe paths remain rejected. Path-only lookup still rejects multiple semantic identities because it has no explicit selection. A regression failed against the previous code and passes with the fix; it reproduces the identical-content Contribution/Candidate case and checks each rejection boundary. Before attempt 16, all 181 scoped Rust tests, 24 relevant Python tests, scoped Clippy and Ruff passed.

During validation, newly compiled Rust and C executables also stalled before entering their programs. macOS `syspolicyd` logs subsequently recorded completed Gatekeeper/XProtect scans and waking the waiting launches. The user confirmed that no security dialog was visible. The programs and test suite ran successfully after those scans completed; no host security setting was disabled. The aborted first validation run is not a test failure or a Swarm run.

## Remaining delivery boundary

The evaluation driver stages integration and independent review after the adaptive loop reaches Contributions. Full production autonomy requires the model-directed planner to own that continuation, including exact check feedback, finding resolution, acceptance and delivery under the existing authority rules. JEV remains the optional application-layer advisor described by ADR 0043; this evaluation uses a separate Codex reviewer.

## Successful fresh run

Attempt 16 completed the entire sequence in one Room: two accepted Contributions, 96.426 seconds of native overlap, a Candidate citing both Contributions, 26 passing fixed checks, a fresh independent passing review, applied writeback, and an accepted Result. Cleanup succeeded and measured identities stayed unchanged. The original implementation behavior was retained; the integrator changed a comment. The artifact-alias fix has a passing regression, but this successful Candidate had different bytes from the implementation Contribution, so attempt 16 itself did not exercise identical-content aliasing.

The seventh adaptive Invocation omitted the required planning schema envelope. No worker Action was dispatched from it. Its successor received the retained `provider_output_invalid` outcome, returned the complete schema envelope and selected the intended claim successfully. This demonstrates bounded recovery without an operator repair. It does not prove a learned strategy or show that the model identified the exact cause; malformed-output feedback is still generic at this boundary.

Agent B's 26 authored tests also pass unchanged against the delivered file in a supplemental sandboxed run after Room completion. The tests were available to integration but were not executed as part of the live gate. Moving that execution into the production check flow is a concrete improvement: an authored test artifact should become executable evidence before acceptance, subject to the same isolation and identity controls.

The full wrapper took 637.920 seconds, with 13 adaptive and four delivery Invocations. The implementation native turn lasted 119.378 seconds and test authoring 96.426 seconds. The measured overlap proves concurrent useful computation, but setup, metadata, model scheduling, integration, review and supervision dominate the overall elapsed time. There is no equivalent serial baseline, so no speedup factor is claimed.
