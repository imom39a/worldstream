# Agent Swarm ticket breakdown

Status: Draft for breakdown approval. The specification is accepted; no Linear
issues have been published for this breakdown.

Destination: Linear team **Imom39a (IMO)**, project **WorldStream**. Publish one
issue per approved slice using native blocked-by relationships. Apply the
configured ready-for-agent label; only the dependency frontier is executable.
Do not create or modify a parent issue.

Source: [accepted specification](agent-swarm-spec.md). Full issue drafts are
linked below, one file per ticket. Their numbers are draft references and will
be replaced with Linear identifiers after approval.

## Breakdown

1. **Create and inspect a local Swarm in the TUI**
   - **Blocked by:** None — can start immediately.
   - **What it delivers:** Create a goal and explicit roster in local SQLite, then reopen the same Room from a native TUI.
   - [Full draft](../.scratch/agent-swarm/issues/01-create-local-swarm.md)

2. **Run local workers that claim work and publish contributions**
   - **Blocked by:** #1.
   - **What it delivers:** Two controlled workers compete for tasks and publish attributable, openable local contributions.
   - [Full draft](../.scratch/agent-swarm/issues/02-claim-and-contribute.md)

3. **Integrate contributions and accept a reviewed Swarm Result**
   - **Blocked by:** #2.
   - **What it delivers:** Combine a small report, check it, and accept the exact result after review by another member.
   - [Full draft](../.scratch/agent-swarm/issues/03-integrate-and-review-results.md)

4. **Run signed-in Codex members with explicit model and effort**
   - **Blocked by:** #2.
   - **What it delivers:** Two subscription Codex members complete work and publish contributions using independently selected settings.
   - [Full draft](../.scratch/agent-swarm/issues/04-codex-members.md)

5. **Run signed-in Claude Code members with explicit model and effort**
   - **Blocked by:** #2.
   - **What it delivers:** Two subscription Claude Code members contribute without hidden model fallback or shared effort changes.
   - [Full draft](../.scratch/agent-swarm/issues/05-claude-members.md)

6. **Run signed-in Kiro members with independent model and effort**
   - **Blocked by:** #2.
   - **What it delivers:** Kiro members contribute through subscription CLI/ACP without changing one another’s settings.
   - [Full draft](../.scratch/agent-swarm/issues/06-kiro-members.md)

7. **Coordinate dependent Swarm Work Items**
   - **Blocked by:** #2.
   - **What it delivers:** Agents build a bounded plan whose dependencies determine which work can start and what is blocked.
   - [Full draft](../.scratch/agent-swarm/issues/07-dependent-work.md)

8. **Correct rejected results and resolve review disputes**
   - **Blocked by:** #3, #7.
   - **What it delivers:** Review findings create corrective work, and unresolved disagreements cannot be hidden by later approval.
   - [Full draft](../.scratch/agent-swarm/issues/08-review-corrections.md)

9. **Pause, Stop and detach owned local workers**
   - **Blocked by:** #2.
   - **What it delivers:** Detach keeps work running, Pause drains current turns, and Stop cleans up owned processes on both systems.
   - [Full draft](../.scratch/agent-swarm/issues/09-pause-stop-detach.md)

10. **Recover interrupted Swarms without blindly repeating effects**
   - **Blocked by:** #9.
   - **What it delivers:** Restart restores the same swarm, reconciles interrupted attempts and waits for explicit Resume.
   - [Full draft](../.scratch/agent-swarm/issues/10-recover-and-reconcile.md)

11. **Steer the whole Swarm or specific work from the TUI**
   - **Blocked by:** #3, #7, #10.
   - **What it delivers:** Send advisory Suggestions or binding Directions that follow affected work across reassignment.
   - [Full draft](../.scratch/agent-swarm/issues/11-human-steering.md)

12. **Schedule and claim Swarm Progress Reviews**
   - **Blocked by:** #11.
   - **What it delivers:** Quiet or blocked swarms produce one durable review item that an existing member can pick up.
   - [Full draft](../.scratch/agent-swarm/issues/12-scheduled-progress-reviews.md)

13. **Realign stalled work and escalate repeated failed correction**
   - **Blocked by:** #12.
   - **What it delivers:** A progress reviewer creates corrective work and blocks only the affected chain after repeated failure.
   - [Full draft](../.scratch/agent-swarm/issues/13-bounded-realignment.md)

14. **Share provider capacity and budgets across Swarms**
   - **Blocked by:** #12.
   - **What it delivers:** Concurrent swarms obey global caps, fair default priority and optional budgets that request Pause.
   - [Full draft](../.scratch/agent-swarm/issues/14-shared-capacity-budgets.md)

15. **Preserve newer shared resources during result integration**
   - **Blocked by:** #3, #7.
   - **What it delivers:** Human or other-swarm edits survive integration, with affected work refreshed or visibly blocked.
   - [Full draft](../.scratch/agent-swarm/issues/15-shared-resource-conflicts.md)

16. **Deliver a reviewed code change with test evidence**
   - **Blocked by:** #10, #15.
   - **What it delivers:** The same swarm engine that produced a report now produces and applies an authorized, tested code change.
   - [Full draft](../.scratch/agent-swarm/issues/16-reviewed-code-change.md)

17. **Quiesce completed Swarms and reopen the same goal**
   - **Blocked by:** #12, #15.
   - **What it delivers:** Completion stops automatic work; explicit reopening creates a newly reviewed result without losing history.
   - [Full draft](../.scratch/agent-swarm/issues/17-complete-and-reopen.md)

18. **Install and update the local TUI on Windows and macOS**
   - **Blocked by:** #10.
   - **What it delivers:** Launch a distributable outside a checkout, find saved swarms and update without losing local data.
   - [Full draft](../.scratch/agent-swarm/issues/18-native-packaging.md)

19. **Assess Swarm health with JEV without granting it authority**
   - **Blocked by:** #12, #18.
   - **What it delivers:** An opt-in application-layer advisor attaches fresh typed goal-alignment and progress signals to ordinary reviews without becoming an agent or authority.
   - [Full draft](../.scratch/agent-swarm/issues/19-jev-swarm-assessments.md)

20. **Qualify ten-agent mixed-provider Swarms on Windows and macOS**
   - **Blocked by:** #4, #5, #6, #8, #13, #14, #16, #17, #19.
   - **What it delivers:** Packaged Agent Swarm completes the agreed report and coding scenarios with roughly ten mixed-provider members.
   - [Full draft](../.scratch/agent-swarm/issues/20-mixed-provider-qualification.md)

## Dependency rationale

- Start with one native local create/inspect path, then add controlled worker
  ownership/contributions. The first reviewed goal result is slice 3.
- Provider slices 4–6 share only the contribution contract from slice 2 and can
  run in parallel. They do not wait for each other or for the reviewed-result
  implementation; each demonstrates its real contribution-producing flow.
- Lifecycle and work dependencies branch from slice 2. Packaging includes
  preserving recovery behavior through updates and therefore follows slice 10.
  Existing Room and Runner contracts provide the starting seams; perform only
  bounded, necessary prefactors within the relevant slice.
- Progress review creation requires actual steering/control/dependency behavior.
  Steering follows reconciled ownership handoff from slice 10. Scheduling
  follows review creation in slice 12, which already includes recovery as a
  transitive prerequisite; slice 14 needs only that direct edge.
- JEV follows the durable review path and native packaging so its isolated
  helper can be qualified on both systems. It is opt-in and fails open to the
  ordinary review flow; it is not a roster provider or a source of Room
  authority.
- The release qualification is a join of finished behavior and provider support,
  with approximately ten members, optional JEV assessments, and both coding
  and non-coding outcomes.
  It does not conceal unfinished feature implementation.

## Existing tracker context

- No Agent Swarm or subscription-CLI issues matched the project searches.
- [IMO-227](https://linear.app/imom39a/issue/IMO-227/measure-action-starvation-during-slow-agent-decisions-in-changing)
  is Done and supplies contention evidence. Slice 2 reuses that knowledge and
  preserves exact-Head admission; it does not recreate the measurement project.
- [IMO-217](https://linear.app/imom39a/issue/IMO-217/keep-live-participant-action-state-fresh-after-private-room)
  is Backlog. Its live-client hidden-head correction is related, not a blanket
  blocker: the Swarm path must refresh and re-evaluate current authorized
  state before submission and explicitly handle genuine concurrent rejection.
- [IMO-231](https://linear.app/imom39a/issue/IMO-231/specify-durable-external-effect-requests-and-reconciliation-for)
  is Done for a contract and fake-target prototype. Slice 10 implements durable
  recovery for the concrete Swarm flow rather than treating that prototype as
  a finished execution service.
- [IMO-147](https://linear.app/imom39a/issue/IMO-147/post-mvp-hardening-for-cli-managed-installations)
  remains a broader CLI-hardening backlog. Native Swarm behavior is covered by
  these slices without closing or expanding that existing issue.

## Review before publishing

Confirm whether the slice granularity is appropriate, whether each blocker is a
genuine prerequisite, and whether any tickets should be merged or split.
The to-tickets workflow requires approval of this breakdown before publication.
