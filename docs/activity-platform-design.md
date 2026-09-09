# Activity Platform experience design

Status: Implementation direction, 2026-09-07; stabilization priority, 2026-09-08

## Current milestone: platform usability and stability

The next MVP milestone is a usable Hosted Activity Platform tested with the
existing Agent Heist experience. It covers game setup, entry, exit, re-entry,
game-specific results, and repeated use. Heist gameplay improvements belong to
a later, separate session. Basic client defects that prevent participation are
still platform-usability blockers; better agent strategy, game balance, and
new game rules are not this milestone's goal.

The user accepted decisions Q1–Q10 below and confirmed the shared understanding
on 2026-09-08. They define the stabilization scope, not a completed release.
The specification, test boundaries, and ten-slice ticket plan are approved and
published in Linear. See the implementation handoff below. Existing authority,
privacy, and spending boundaries remain in force; Q10 explicitly narrows
disaster-recovery qualification for this hobby preview.

### Platform-wide scope

These are generic Hosted Activity Platform operations, not Agent Heist
features. Setup, invitations, account navigation, entry, Run Re-entry, capacity
reconciliation, and verified result publication must work through the reviewed
Listing, Host, and client contracts. Agent Heist is the first acceptance
activity, not a special case in those shared operations.

Activity Packs retain their rules, Actions, phases, visibility, and Outcomes.
Activity Clients retain game-specific presentation and interaction. Each
Listing's reviewed Result Projector Revision supplies terminal and result
interpretation; shared code must not assume Heist phases, three seats, a numeric
score, or a winner. Other activities can reuse the platform when they satisfy
these contracts. This does not make every existing Pack eligible for hosted
launch without its required integration and review.

Preserve the ADR 0017/0019 client boundary during this work. The shared shell
must not import a Heist presentation component or add Pack-name branches to
implement navigation, spectating, or lifecycle operations. Existing coupling is
implementation debt to correct, not an extension of the product contract.

### Accepted interview decisions — round 1, 2026-09-08

**Q1 — Leaving the screen keeps the seat.** Use **Back to games** for navigation,
not an ambiguous **Leave game** control. Navigation, closing the tab, or signing
out does not depart the Membership, pause the Activity, or assign a replacement
agent. Run Re-entry restores access to the same Activity Run and Membership;
the Activity may have advanced or finished during the absence. Explicit
pre-start cancellation and Seat Claim release remain separate operations under
their existing safety boundaries. Generic mid-game forfeiting is outside this
milestone.

**Q2 — My games is the primary return path.** After sign-in, users can find
activities they created or joined, including their account-controlled external
agent participation. Show the appropriate next action: **Continue setup**,
**Return to game**, or **View result**. Sign-in preserves the intended eligible
destination instead of requiring a saved waiting-room URL. This navigation
uses existing Launch Requests and Activity Runs; it is not another authority
for Room State. Public spectator links remain separate and confer no
participant access. **My games** is a UI label, not a new domain entity.

**Q3 — Results and history precede ranking.** This stability release requires
game-specific results and personal activity history, not a ranked leaderboard.
Results show the Pack-defined Outcome, score when applicable, game version, and
exhibition status under the existing verified-publication and privacy rules.
No missing result is treated as a zero score. Ranking, score-comparison
eligibility, and team-versus-individual credit require a later decision. This
preserves the Recent Results boundary in ADRs 0021 and 0023.

### Accepted interview decisions — round 2, 2026-09-08

**Q4 — Freeze the existing Heist rules as the acceptance baseline.** Use Heist
0.3 with the necessary Activity Client usability fixes. The updated client
already declares support for that exact Pack Revision. Publish a new reviewed
compatible Listing and client binding; do not rewrite a retained Listing or
silently change an existing Run's selected client. Preserve the unshipped 0.4
gameplay work for the separate gameplay session. This selects a test baseline,
not Heist-specific behavior for the shared platform.

**Q5 — Keep failed and unfinished setup visible.** My games retains setup
attempts and shows their confirmed condition: waiting for participants,
checking setup, ready to enter, cancelled, or unable to start. Recovery resumes
the same Launch Request and Room Setup Operation. A timed-out response is not
proof that no Room exists and does not authorize a replacement. Offer a fresh
attempt only after the previous one is safely resolved. Cancelled and
unsuccessful attempts remain in personal history, separately labelled from
played-game results.

**Q6 — Recover connections without repeating uncertain Actions.** A temporary
network interruption may trigger bounded automatic reconnection. When the
Browser Activity Session cannot be restored, expose **Return to game** for
fresh authorized Run Re-entry to the same Membership. One explicit re-entry
click after a server restart is acceptable for this MVP; authentication is
required again only when the platform sign-in has expired. Activity Actions
remain disabled until current authorized synchronization is installed. Neither
reconnection nor re-entry automatically repeats an uncertain gameplay Action.

**Q7 — Distinguish the Outcome from its verified publication.** The Activity
Client may show the Outcome supplied by its authorized Pack view before the
platform has finished verifying the publishable result. The platform page
updates to the verified result without a manual refresh. While publication is
delayed, show an honest pending or unavailable state and allow return through
My games. A pending label requires authorized evidence; privacy or integrity
suppression must retain the existing generic unavailable presentation. A
missing result is neither a loss nor a zero score.

### Accepted interview decisions — round 3, 2026-09-08

**Q8 — Repeat play requires safe automatic cleanup.** Ordinary completed games
must not require offline operator retirement before users can start another.
Stop and fence completed House Runner units, release capacity only from
confirmed lifecycle evidence, and preserve Canonical History, assignment
identity, and consumed allowance evidence. Uncertain setup remains recoverable
and may temporarily block a new launch. Closing a browser is not completion
evidence and never frees its capacity. A budget limit may make House fill
unavailable; cleanup does not reset that limit.

**Q9 — Qualify the user journey, not model skill.** Require three consecutive
local matches using scripted participants and the fake provider, plus one
tightly capped real-LLM match on the same deployed source revision. Exercise
setup, invitations, refresh, leaving and returning, server restart, spectators,
results, and starting another game without manual repair. Include actual
browser checks rather than relying only on API tests, and non-Heist contract
fixtures to detect game-specific assumptions without adding another hosted
game to this milestone. Keep the existing privacy, direct-WebSocket, and
supported-device acceptance checks. Models need not win. Local build and
publishing are sufficient; CI/CD is not a prerequisite.

**Q10 — Defer populated disaster recovery, not ordinary durability.** Normal
same-volume restart and Run Re-entry remain mandatory. Full populated
Runtime–Controller–platform restore qualification is deferred, with a public
warning that catastrophic storage loss may require an operator-approved fresh
preview and loss of history. Retain protected backups without calling them
verified restores. The earlier empty-installation drill cannot satisfy the
deferred populated-recovery gate, which remains separate outstanding work.
This does not authorize resetting the current installation. The explicit
release-contract amendment is
[ADR 0026](adr/0026-defer-populated-disaster-recovery-for-the-hobby-preview.md).

### Implementation handoff — published 2026-09-08

The canonical specification is
[IMO-187](https://linear.app/imom39a/issue/IMO-187/stabilize-the-generic-hosted-activity-platform-mvp).
Linear owns ticket descriptions, current status, and native blocking links.
The table maps the approved slices to their published issues; it is not a
second live tracker or evidence that any slice is complete.

| Slice | Published ticket | Delivery |
| --- | --- | --- |
| S1 | [IMO-185](https://linear.app/imom39a/issue/IMO-185/make-live-heist-plan-selection-usable-without-typing-truncated-ids) — reused | Usable Heist 0.3-compatible client |
| S2 | [IMO-188](https://linear.app/imom39a/issue/IMO-188/find-owned-and-joined-activities-in-my-games) | Account-scoped My games |
| S3 | [IMO-189](https://linear.app/imom39a/issue/IMO-189/open-independent-activity-clients-with-safe-platform-navigation) | Independent client entry and return |
| S4 | [IMO-190](https://linear.app/imom39a/issue/IMO-190/restore-the-same-participant-after-disconnect-or-server-restart) | Same-Membership reconnect and Run Re-entry |
| S5 | [IMO-191](https://linear.app/imom39a/issue/IMO-191/recover-incomplete-setup-without-duplicating-an-activity-run) | Idempotent setup recovery |
| S6 | [IMO-192](https://linear.app/imom39a/issue/IMO-192/retire-house-runner-units-and-reuse-capacity-automatically) | Automatic safe House retirement and capacity reuse |
| S7 | [IMO-193](https://linear.app/imom39a/issue/IMO-193/cancel-or-abandon-an-unstarted-activity-safely) | Safe pre-start cancellation and abandonment |
| S8 | [IMO-194](https://linear.app/imom39a/issue/IMO-194/complete-the-live-to-result-and-personal-history-journey) | Verified results and personal history |
| S9 | [IMO-195](https://linear.app/imom39a/issue/IMO-195/qualify-a-repeat-play-candidate-and-prepare-manual-publication) | Three-match local qualification and manual-release preparation |
| S10 | [IMO-184](https://linear.app/imom39a/issue/IMO-184/prove-and-record-the-authoritative-hosted-mvp-acceptance-gate) — reused | Exact deployed-candidate acceptance |

At publication, the implementation frontier is IMO-185, IMO-188, IMO-191, and
IMO-192. Start with IMO-191's unexplained formation failures, then IMO-192's
repeat-play cleanup. This is a suggested execution order, not an extra blocking
edge. Recheck native blockers before starting any ticket. Do not treat the
specification issue as another implementation task.

IMO-184 retains its original IMO-163 parent and now explicitly follows the
accepted local-first gate and ADR 0026. Neither parent is closed or rewritten
by publishing this plan. The existing gameplay work in
[IMO-186](https://linear.app/imom39a/issue/IMO-186/qualify-bounded-heist-agent-gameplay-after-the-house-submission-repair)
is preserved in Backlog. Populated disaster recovery is separately tracked in
[IMO-196](https://linear.app/imom39a/issue/IMO-196/qualify-populated-hosted-disaster-recovery-after-mvp-stabilization).
Neither deferred item blocks stabilization.

This publication changes planning records and documentation only. It does not
qualify a build, fix an outstanding failure, deploy, or authorize extra spend.

## Product shape

The Hosted Activity Platform is a destination for many Activity Packs. Agent
Heist is one activity in that library. Its story, imagery, roles, and gameplay
must not become the identity of the whole platform.

Use the sense of place, strong typography, and recognizable game identity of
[Shards](https://play-shards.com/) and the illustrated environments and direct
invitation to play of [AI Dungeon](https://aidungeon.com/) as visual inspiration.
Give this product its own assets and composition. The shared shell should feel
like a game library; each Activity Client should feel like entering the selected
activity.

This direction preserves the boundaries in ADR 0017 and ADR 0019. Presentation
does not establish availability, Action legality, private knowledge, or Outcome.

## Three layers

| Surface | Shared behavior and design | Activity-specific presentation |
| --- | --- | --- |
| Discovery | Platform identity, navigation, library layout, availability, sign-in, responsive controls | Cover artwork, activity title, description, participation information, recorded-demo links where available |
| Start and join | Authentication, role selection, invitations, House Agent availability, waiting-room status, entering the selected client | Role labels and participation choices supplied by the reviewed listing; optional restrained cover treatment |
| Play and watch | Consistent focus visibility, readable controls, connection feedback, honest data modes | Full visual world, layout, role identity, private information, Actions, Activity Phases, and Outcome, owned by the Activity Client |

The shared shell must not assume that every activity has a crew, a heist,
three seats, combat, a score, or a win condition. New activities must receive a
neutral title treatment until they have their own artwork; they must never
inherit Agent Heist artwork by default.

## Shared platform

- Lead with activity discovery. An illustrated crossroads gives the platform
  its own setting: several distinct activity environments can coexist. Keep a
  direct Explore activities link and the collection immediately below it.
  This decorative scene is not another Activity Pack or a playable Room.
- Use warm serif display type in discovery and artwork-led activity covers.
  Individual clients retain their own type, color, and layout. Agent Heist's
  condensed tactical lettering does not define the platform or Negotiate.
- Use deep ink surfaces, warm primary controls, a restrained cyan accent,
  large readable display typography, and clear focus states.
- Give available and upcoming activities equal visual quality. Derive whether
  users can enter from the catalog service, not the artwork or local metadata.
  When live Agent Heist is unavailable, its explicitly recorded demo stays
  accessible as a separate entry. It does not pretend to launch a live Room.
- Keep sign-in, joining, and waiting-room language neutral: activity, role,
  participant, and seat. House Agents appear only when the listing allows them.
- Keep technical identities available in disclosures. Put the information a
  person needs for their next decision first.
- Label results by activity. The current API only supplies Agent Heist recent
  results; this is not a universal leaderboard or an all-activity feed.
- Keep the developer manual available without making it the main player journey.

## Individual Activity Clients

### Agent Heist

A covert-operations setting: a rainy city, vault architecture, cyan intelligence
accents, amber decisions, and distinct role panels.

The play surface leads with the current operation and Activity Phase. Crew
presence sits beside the board; the participant's private intel and offered
Actions sit in their own panel. The center shows public clues, proposed plans,
sealed commitment counts, and the actual Outcome when present.

Recorded playback retains an explicit offline notice and user-controlled
playback. A recorded view selector must never appear in a live client.

### Negotiate

A negotiation-table setting, with burgundy, walnut, copper, and paper-like
document treatments. Its cover should feel distinct from Agent Heist.

The client centers on the current agreement and the
participant's next decision. Role-specific review and approval belong beside
the agreement. Exact document bytes, signature evidence, and verification remain
available because those are meaningful to this activity; they should be
organized around the decision rather than spread across generic status cards.

Do not invent factions, battles, points, or a diplomacy game on top of the
existing negotiation rules. Validate the layout against every supported Role
and exact Activity Pack Revision before releasing it.

### Additional packs

For each new pack, define a short visual brief: setting, palette, artwork,
primary activity, first decision, role differences, terminal result, and
spectator experience. Then design its client around those needs.

Reuse accessibility and interaction conventions. Do not force every activity
into the same three-column HUD or merely swap its accent color.

## Delivery sequence

1. Establish the shared library, navigation, entry dialog, waiting-room and result
   styling. Preserve live availability and existing routing.
2. Apply the first complete Activity Client direction to Agent Heist, including
   recorded playback, participants, spectators, and disconnected states.
3. Apply Negotiate’s walnut, burgundy, and copper setting to its independent
   agreement and approval workspace. Use the shared reading foundation in the
   manual, fixture docs, and Inspector.
4. Add discovery filters when the catalog has enough activities and reviewed
   metadata to support useful choices. Do not add fictional packs or inactive
   search controls to make the library look populated.
5. Expand cross-activity results only when the platform supplies the necessary
   activity-scoped indexes. Preserve each activity's own result meaning.

## Review criteria

- The home page is recognizable as a platform for several activities.
- Adding another listing does not require new gameplay logic in the shell.
- Entering a pack changes the environment while controls remain understandable.
- The player can identify their role, current phase, and available next decision.
- Recorded, disconnected, unavailable, and live states remain distinguishable.
- A spectator never receives participant-private data or controls.
- Keyboard operation, 320px layouts, readable text, and reduced motion work.
- Artwork is locally bundled; it does not depend on another game's server.

Artwork provenance and generation prompts are retained in
[`web/demos/src/assets/README.md`](../web/demos/src/assets/README.md).

## Shared artifacts and release packaging

The shared palette and type live in `web/design/tokens.css`. The manual home,
articles, search, capability explorer, fixture documentation, Inspector, platform
entry states, and existing social previews follow the same foundation. Pack
artwork and gameplay presentation remain owned by each Activity Client.

Agent Heist v6, Negotiate v3, and Inspector v2 are the current immutable client
releases. The local Host serves their current paths and preserves Agent Heist
v5, v4, v3, and v2 bytes at their retained paths. Current discovery Listing
0.21.0 selects the Heist v6 hosted client; 0.20.0 and older Listings retain their exact
client release and route. Pack revisions and game rules are unchanged.
The catalog migration, Runtime admission list, generated catalog, and client
installer travel with that release change. Hosted publication needs the normal
coordinated database, Runtime, and platform rollout.

## Verification

Use the repository’s pinned Node 24.18.1. Validate the manual’s routes, UI tests,
and production build; the platform’s tests and complete production packaging;
the three clients’ type checks, tests, and builds; exact release identities and
retained asset serving; and the affected Host binding and handoff tests. Verify
the catalog migration against local Postgres and preserve the old Listing rows.

Browser review covers readable guides, search, copy feedback, capability filters,
mobile navigation at 320px, recorded playback, and client connection states.
A read-only design preview does not establish live service availability.
