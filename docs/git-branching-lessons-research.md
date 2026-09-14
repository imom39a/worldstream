# Git's branching design: lessons for WorldStream

Research checked 2026-09-13. This is a non-normative research note. It proposes no change to `CONTEXT.md`, accepted ADRs, codecs, or the current protocol. The user is evaluating independently progressing agent groups that can branch, compare results, and integrate useful work.

## Assessment

Git provides strong implementation evidence that immutable shared ancestry, independently advancing heads, and explicit integration can support extensive concurrent work. It does not establish that agent voting finds the best result, that arbitrary Activity State can be merged, or that branching reduces total cost. Those are separate hypotheses to measure.

The useful product direction is **preserve alternatives while agents make progress, then require evidence at integration boundaries**. Branching moves coordination from every speculative step to selected integration points. It can reduce immediate contention and preserve minority approaches. It can also accumulate incompatible assumptions and duplicate expensive work. A bounded exploration policy and a domain validator are as important as the history graph.

## What was already present in the original implementation

Git's first recorded commit, `e83c5163316f89bfbde7d9ab23ca2e25604af290`, separates an immutable content-addressed object database from a mutable current-directory cache. Trees reference other objects; identical subtrees can share their representation. Its README explicitly separates integrity from provenance and validity. This is unusually direct evidence that the durable data model should remain simpler than the agents using it. [Original README](https://github.com/git/git/blob/e83c5163316f89bfbde7d9ab23ca2e25604af290/README)

The initial `commit-tree.c` already accepts multiple parents and includes them, in order, in the commit content. Its comment observes that changing parent order changes commit identity even if the tree is identical. A history node therefore records both a resulting value and how that value relates to prior work; equal values need not mean equal histories. [Original commit construction](https://github.com/git/git/blob/e83c5163316f89bfbde7d9ab23ca2e25604af290/commit-tree.c#L86-L147)

Do not copy the initial byte format. That first README hashes compressed object bytes; modern Git's documented object identity is based on the object header and content before compression. Modern merge algorithms, commit-graph indexes, partial clones, and reftable are later work. The historical evidence supports the separation of concerns, not treating every modern capability as a feature of the first implementation. [Original README](https://github.com/git/git/blob/e83c5163316f89bfbde7d9ab23ca2e25604af290/README), [modern object construction](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects#_object_storage)

## Lessons to adopt and limits to preserve

### 1. Share immutable content; keep live execution separate

Modern Git commits point to snapshot trees and parent commits. Trees point to blobs and subtrees. Reusing object identities shares unchanged content without copying an entire history for each branch. This is a logical snapshot model; it does not require physically storing a new full repository for every commit. [Git objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects)

Git worktrees demonstrate a separate concern: multiple working directories share the repository while maintaining their own `HEAD` and index. A worktree is an execution/editing surface, not a distinct identity for each historical alternative. [Git worktree](https://git-scm.com/docs/git-worktree)

**WorldStream inference:** keep a branch's durable ancestry and current Head separate from its Runner processes, model contexts, leases, and UI. A dormant branch should not require a resident Runner. Many agents can participate in one branch; one agent can inspect several authorized branches. A new participant should obtain a bounded authorized current view of its chosen Room, with referenced evidence fetched only when needed.

An exact fork requires more than a board array: pin the parent Complete Head, exact Activity Pack Revision, current state, semantic time policy, timers, random-state policy, authorization, and external-input policy. An identical Activity State hash does not prove two Rooms have interchangeable Memberships, deadlines, or admissible future inputs.

### 2. A branch head is small, but moving it is a correctness boundary

`git update-ref <ref> <new> <old>` updates a reference only when its current value equals the expected old object. It also supports transactions involving multiple reference updates. The small pointer is the mutable part; creating an object and selecting it as the current branch are distinct operations. [Git update-ref](https://git-scm.com/docs/git-update-ref)

**WorldStream inference:** branch creation and result integration need stable operation identities and exact expected source/target Heads. Preparing a candidate must not grant authority to install it later. If either relevant Head or authority changes, revalidate or return a conflict. WorldStream already has a stronger application-specific commit boundary in [ADR 0006](adr/0006-backend-neutral-atomic-room-commit.md): receipts, canonical state, timers, observations, and Activation consequences advance together.

Git's ability to move a branch reference backward is **not compatible** with silently rewriting an existing WorldStream Room. [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md) requires immutable Genesis and ordered Transitions. An integration would need a new authorized Transition, a new Room with explicit provenance, or another versioned contract. Repointing the existing Room's Head at another Room's history would break current semantics.

### 3. Merge starts with a base, not a vote over two final snapshots

Git finds best common ancestors and compares the base, each side, and their changes. There may be multiple equally good merge bases after criss-cross merges. The documented default `ort` strategy handles two heads and may build a reference tree from multiple bases. This complexity already occurs in ordinary source history. [Git merge-base](https://git-scm.com/docs/git-merge-base), [merge strategies](https://git-scm.com/docs/merge-strategies)

**WorldStream inference:** any proposed integration must retain its common basis and the assumptions each side changed or relied on. A JSON union of final Activity States loses those dependencies. Even disjoint writes can conflict: one branch spends a resource another branch assumed remained available; two branches may each assign the same person to different fields while violating a global capacity rule.

Use distinct operations instead of promising one universal merge:

| Intended result | Candidate behavior | Evidence needed |
| --- | --- | --- |
| Choose one successful future | Select an Outcome or a validated candidate path | Goal validator, exact source Head, target authority |
| Reuse an experiment | Import an immutable Artifact or declared finding | Provenance, authorized visibility, current applicability |
| Apply a planned sequence | Re-execute proposed Actions against the target's current state | Preconditions and legality at every applied step |
| Combine compatible contributions | Activity Pack-defined structured integration | Common basis, dependencies, invariants, deterministic result |
| Keep both alternatives | Continue separate Rooms | Explicit resource budget and branch ownership |

The last row is a useful result. Forcing every branch to merge recreates the premature agreement problem that branching was meant to reduce.

### 4. A clean textual merge is not a semantic proof

Git allows content-specific merge drivers. Its standard text merge operates on file content, and a conflicting binary merge can retain one side while leaving the path conflicted. Git does not supply application invariants such as puzzle legality or financial conservation. [Git attributes and merge drivers](https://git-scm.com/docs/gitattributes#_performing_a_three_way_merge)

Git's own conflict workflow separates constructing a resolution from committing it. `git merge-tree` can calculate a merge without updating the working tree or index, which is a useful precedent for a reviewable candidate integration. [Git merge](https://git-scm.com/docs/git-merge#_how_to_resolve_conflicts), [Git merge-tree](https://git-scm.com/docs/git-merge-tree)

**WorldStream inference:** expose a bounded integration preview that names the base, both Heads, exact Pack revision, proposed domain changes, failed preconditions, and validation result. A Participant or policy may select a proposal, but the selected proposal must still pass the Pack and Core reducers at commit. Voting establishes a preference under a declared electorate; it does not establish factual truth.

Git's `rerere` stores previous conflict resolutions and can apply them to matching conflicts. A WorldStream analogue could propose previously successful domain resolutions, keyed to exact Pack semantics and relevant preconditions. It should be an aid to candidate generation; current validation still decides admissibility. [Git rerere](https://git-scm.com/docs/git-rerere)

### 5. Physical compression and logical history are different layers

Git packfiles can represent similar objects using deltas. Their physical bases can differ from historical parent relationships, and repacking can change storage without changing logical object identities. Consequently, a historical DAG edge and a compression dependency are different graphs. [Git packfiles](https://git-scm.com/book/en/v2/Git-Internals-Packfiles)

**WorldStream inference:** retain canonical history identity independently of checkpoint placement, byte compression, and shared physical storage. [ADR 0034](adr/0034-compact-canonical-transition-format.md) proposes compact Transition records with exact recorded inputs and hashes; its production implementation is still a follow-up under that ADR. That is not the same thing as applying Git packfile compression to existing complete-state records. Both can help, but neither defines branching authority or merge legality.

Immutable references can make a fork's metadata small. Actual fork availability still requires complete, cut-consistent continuation witnesses. [ADR 0031](adr/0031-verified-checkpoint-recovery.md) explicitly documents the remaining checkpoint limitations for Rooms with timers, observations, or changed Membership generations. Cheap ancestry references do not remove those requirements.

### 6. Scale comes from indexes and maintenance, not the DAG shape alone

Git's commit-graph is a supplemental, discardable index over existing objects. Parent positions and generation numbers accelerate graph traversal and ancestry queries. Split commit-graph chains avoid rewriting the full index for every increment and periodically consolidate layers. This is later optimization work, separate from the authoritative objects. [Commit-graph design](https://git-scm.com/docs/commit-graph)

The reftable design describes the cost of many loose references and rewriting/scanning packed references. It uses indexed sorted blocks and supports efficient changes to small subsets. Its reported examples are evidence of a particular reference-storage problem and solution, not capacity guarantees for WorldStream. [Reftable design](https://github.com/git/git/blob/master/Documentation/technical/reftable.adoc)

Git partial clone omits selected objects and fetches them on demand; it explicitly distinguishes this from restricting commit history. It depends on an available source for missing objects. [Partial clone design](https://git-scm.com/docs/partial-clone)

**WorldStream inference:** separately measure total branches, actively executing branches, retained Transitions, immutable Artifact bytes, fork latency, merge-base query latency, validation cost, and subscriber fanout. An authorized paginated branch index can show goal, latest verified progress, current Head, participants, and resource use without sending every branch's history into every agent context. A reference to an Artifact must never bypass its access policy.

No Git source reviewed establishes that an unbounded number of model-driven alternatives is cheap. With branching factor `b` over `d` independent choices, naive exhaustive exploration can grow as `b^d`. Cap active branches, depth, compute, and evidence retention independently. Suspend or archive unpromising exploration; preserve enough immutable evidence to explain decisions.

### 7. Reachability is useful for retention, but live readers need protection

Git GC retains objects reachable through several roots, including branches, tags, indexes, and reflogs. The manual also documents a concurrency hazard: an object being used by a writer but not yet referenced can be deleted. Age-based protections mitigate this without claiming a complete solution. [Git garbage collection](https://git-scm.com/docs/git-gc#_notes)

**WorldStream inference:** define retention roots for active Rooms, branch bases, promoted evidence, backups, and in-flight integrations. Use explicit durable retention protection around fork and integration preparation. Deleting a discovery label must not silently delete evidence needed for recovery or Replay. Current canonical-retention rules remain in force; adopting reachability-based deletion would require a separate ADR. A cheap fork that depends on shared objects also creates a long-lived retention obligation.

### 8. Hash integrity is neither validation nor consensus

`git fsck` checks object connectivity and format/integrity properties. Even its term “validity” refers to repository objects, not whether the code solves a problem. The original README similarly states that a consistent blob/tree does not establish its origin or semantic validity. [Git fsck](https://git-scm.com/docs/git-fsck), [original README](https://github.com/git/git/blob/e83c5163316f89bfbde7d9ab23ca2e25604af290/README)

**WorldStream inference:** retain four distinct claims: bytes match a named identity; history follows the declared lineage; Pack execution establishes domain legality; a goal validator establishes the desired result. Agent votes are additional evidence about preference or confidence. A Git-style object DAG contains no algorithm that makes distributed authorities agree on which Head is authoritative. WorldStream should retain its existing guarded commit authority unless an explicit distributed authority protocol is designed and qualified.

## Applied to Hanoi and the product experiment

Hanoi is suitable for demonstrating reversible exploration and exact legality. It is weaker as a demonstration of combining independent contributions: one physical disk cannot occupy both alternative destinations. Selecting a verified path is natural; merging two boards by union is meaningless. Reusing an intermediate plan requires rechecking it against the target board and preserving the order of moves.

Use Hanoi to demonstrate agents choosing a different path, independent progress, observers comparing branches, late joining through bounded state, and selection of a verified successful path. Add a second small Activity Pack where contributions can really combine—for example, independent constraints or subresults feeding a deterministic solution validator. That second experiment tests semantic integration rather than merely selecting a winner.

External effects need a separate boundary. A speculative branch cannot undo an email, purchase, or physical move by returning to an earlier state. [ADR 0035](adr/0035-durable-external-effect-reconciliation.md) already separates committed intent, delivery attempts, idempotency, and uncertain outcomes. A branching experiment should default to simulated effects and explicitly authorize any promotion into effectful execution.

## Suggested research sequence

1. Specify one fork contract with exact parent Head, Pack identity, continuation witnesses, authorization, timer behavior, and effect policy. Preserve each Room's ordered Canonical History while recording cross-Room provenance separately.
2. Implement a throwaway experiment with bounded alternatives, explicit branch selection, and integration previews. Compare a single shared path, voting on each move, and independent branching under equal model/tool budgets.
3. Measure verified success rate, elapsed time, duplicate work, stale Actions, integration conflicts, context bytes, and total spend. Report both failed integrations and branches that produced reusable evidence despite not winning.
4. Test three conflict classes: same domain resource, hidden read/write dependency, and changed target Head during integration. Include equivalent states with different timers or Memberships so an Activity State hash is not accidentally treated as complete identity.
5. Only after these contracts and experiments establish value, choose a kernel ADR and storage implementation. Native Git is a useful reference and may store code Artifacts; it does not replace WorldStream's typed rules, authority, or atomic consequences.

The current protocol explicitly has no fork API, and the current Room lineage hashes one preceding Genesis/Transition. A multi-parent Room Transition or a cross-Room lineage contract is therefore new semantic work. This research supports investigating that change; it does not imply it already exists or authorize rewriting the accepted model.
