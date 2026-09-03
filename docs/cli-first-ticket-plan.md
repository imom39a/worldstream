# CLI-first implementation tickets

Status: Q1–Q18 approved; 12 implementation tickets filed in Linear.
Ticket preparation is complete. IMO-135's contract work, IMO-136's protected
initialization and control admission, and IMO-142's explicit prerequisite imports
are complete. Studio retirement remains gated on the complete replacement proof.

The [approved implementation plan](cli-first-implementation-plan.md) owns the
product contract. [ADR 0018](adr/0018-cli-first-operator-surface.md) records the
boundary, and the [decision record](cli-first-operator-proposal.md) retains
the source audit. Linear owns ticket execution status; this file is a navigation
and dependency snapshot, not a second issue tracker.

## Start here

The completed [IMO-135](https://linear.app/imom39a/issue/IMO-135/freeze-additive-cli-commands-outputs-and-compatibility-fixtures)
freezes the additive command, input, output, and compatibility contracts.
The completed
[IMO-136](https://linear.app/imom39a/issue/IMO-136/initialize-protected-local-state-and-authenticate-all-operator-control)
adds protected initialization and control admission behind the internal preview
gate. The completed
[IMO-142](https://linear.app/imom39a/issue/IMO-142/prepare-approved-agent-and-client-prerequisites-without-studio)
adds exact reviewed local prerequisite imports through the same preview gate.
The current frontier is managed lifecycle (IMO-137) and generic JSON setup
(IMO-139).

Each other ticket has native Linear blocking relations. Start a ticket only
after all its blockers are complete. Update its readiness label at that point;
do not treat the ticket's number or this table as permission to skip blockers.

## Dependency order

| Ticket | Implementation slice | Blocked by |
| --- | --- | --- |
| [IMO-135](https://linear.app/imom39a/issue/IMO-135/freeze-additive-cli-commands-outputs-and-compatibility-fixtures) | Freeze additive CLI commands, outputs, and compatibility fixtures | None — complete |
| [IMO-136](https://linear.app/imom39a/issue/IMO-136/initialize-protected-local-state-and-authenticate-all-operator-control) | Initialize protected local state and authenticate all operator control routes | IMO-135 |
| [IMO-137](https://linear.app/imom39a/issue/IMO-137/implement-recoverable-managed-server-lifecycle-and-bounded-logs) | Implement recoverable managed server lifecycle and bounded logs | IMO-136 |
| [IMO-138](https://linear.app/imom39a/issue/IMO-138/expose-installed-next-start-and-running-pack-state-through-pack-list) | Expose installed, next-start, and running Pack state through pack list | IMO-137 |
| [IMO-139](https://linear.app/imom39a/issue/IMO-139/publish-generic-json-room-setup-schema-defaults-and-runnable-examples) | Publish generic JSON Room setup, schema defaults, and runnable examples | IMO-136 |
| [IMO-140](https://linear.app/imom39a/issue/IMO-140/create-and-resume-rooms-from-immutable-cli-setup-operations) | Create and resume Rooms from immutable CLI setup operations | IMO-139 |
| [IMO-141](https://linear.app/imom39a/issue/IMO-141/make-launch-readiness-client-neutral-and-preserve-declared-pack-start) | Make launch readiness client-neutral and preserve declared Pack start behavior | IMO-140 |
| [IMO-142](https://linear.app/imom39a/issue/IMO-142/prepare-approved-agent-and-client-prerequisites-without-studio) | Prepare approved agent and client prerequisites without Studio | IMO-136 |
| [IMO-143](https://linear.app/imom39a/issue/IMO-143/connect-scoped-clients-and-runners-through-the-cli) | Connect scoped clients and Runners through the CLI | IMO-137, IMO-140, IMO-142 |
| [IMO-144](https://linear.app/imom39a/issue/IMO-144/prove-complete-cli-only-heist-direct-sdk-and-negotiate-flows) | Prove complete CLI-only Heist, direct SDK, and Negotiate flows | IMO-138, IMO-141, IMO-143 |
| [IMO-145](https://linear.app/imom39a/issue/IMO-145/prepare-versioned-cli-first-packaging-and-verify-the-getting-started) | Prepare versioned CLI-first packaging and verify the getting-started guide | IMO-144 |
| [IMO-146](https://linear.app/imom39a/issue/IMO-146/retire-studio-through-the-verified-authenticated-cli-cutover) | Retire Studio through the verified authenticated CLI cutover | IMO-145 |

After protected initialization, lifecycle, JSON setup, and prerequisite-import
work can proceed in parallel. Readiness and connection work meet at the real
reference-flow gate. Packaging prepares the successor artifact format before
the final cutover activates it and removes the frontend.

## Completion ownership

- IMO-135 preserves existing CLI contracts and defines new flags, selectors,
  JSON results, exit statuses, and explicit-input behavior.
- IMO-136 and IMO-137 own protected initialization, complete operator-route
  authentication, verified process ownership, logs, recovery, and the accepted
  managed-Runners restart postcondition.
- IMO-138 distinguishes installed, next-start, and loaded Packs without
  weakening offline approval or retained-lineage protections.
- IMO-139 and IMO-140 own schema constants/defaults, generic examples, immutable
  intent, exactly-once creation, partial results, and operation-only resume.
- IMO-141 owns client-neutral readiness and declared launch. It must not expose
  private participant content or create canonical readiness.
- IMO-142 closes the fresh-installation prerequisite gap: explicit reviewed
  imports, named Profiles and provider references, and approved Runner/client
  dependencies. It does not start processes, silently approve content, or put
  installation trust into Room setup.
- IMO-143 owns scoped credential files, approved Runner controls, and secure
  one-use browser launch.
- IMO-144 proves CLI-only Heist, a direct SDK participant, Negotiate, and the
  existing managed reference integration against its declared compatible Pack.
  The current managed fixture supports Counter v4; do not assume Heist
  compatibility from its binary alone.
- IMO-145 prepares version-aware release inventory and verifies one coherent
  getting-started guide from fresh and retained state.
- IMO-146 removes only Studio's web product after the prior gates pass,
  preserves headless services/state/independent clients, and reruns the complete
  flow against the cutover result.

## Existing work and evidence

Existing implementations are reused, not reopened or replaced wholesale:

- [IMO-78](https://linear.app/imom39a/issue/IMO-78/run-the-complete-studio-driven-agent-heist-story)
  retains its historical Heist acceptance evidence and outstanding proof.
- [IMO-81](https://linear.app/imom39a/issue/IMO-81/run-one-optional-managed-reference-agent-host)
  owns the existing reference Agent Host and remaining real-Runtime proof.
- [IMO-121](https://linear.app/imom39a/issue/IMO-121/deliver-the-production-worldstream-negotiate-experience)
  retains the Negotiate implementation and its live/release evidence obligations.
- [IMO-123](https://linear.app/imom39a/issue/IMO-123/automate-the-expanded-runtime-plus-packs-release-gates)
  retains the release framework and genuine qualification gaps.

These issues are related where relevant, not artificial prerequisites for
reimplementing already available code. Their descriptions, comments, and
relations were inspected; their statuses were not changed. New integration
evidence may satisfy particular old criteria, but each requires an explicit
evidence mapping before a later closure. Local test success is not a signed
release, independent-peer qualification, or an outside-adopter trial.

## Verification of this handoff

All 12 created issues were read back. Their project, descriptions (allowing
Linear's Markdown normalization), status, readiness labels, and native blocking
relations match this plan. The graph has no cycles. At ticket creation, only
IMO-135 was unblocked in this new tranche.

This handoff does not claim that any new command exists. The current
[getting-started guide](getting-started.md) continues to describe executable
current behavior and labels the replacement as pending implementation.
