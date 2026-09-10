# Agent Heist: from protocol forms to a playable game

Status: design proposal and throwaway interaction study, 2026-09-10. Not an
approved production redesign. The live release and current gameplay are unchanged.

## Decision to make

Can a new human understand their goal, make a valid first move, and explain
the result without an engineer or a protocol reference?

Recommend **Mission focus**, with an on-demand shared crew board and an untimed
practice before joining a live Room. Preserve the existing planning game for
the first redesign. Do not promise a character-movement or stealth simulation.

The visual direction is a compact cooperative card game with a cinematic
setting: one current decision, a visible shared clock in live play, and game
objects that carry the technical identifiers internally. Art supports decisions;
it is not a large header above a list of forms.

## What is wrong today

The critique is about interaction and explanation, not merely styling:

- `AgentHeistClientView.tsx` renders every available Action as a separate form.
  It exposes `Clue ID`, `Claim code`, and `Offer ID` as things the human must know.
- The initial private-clue list is empty until inspection, so it does not even
  show the unopened object the player is supposed to inspect.
- The page prioritizes connection, membership, sequence and frame metadata over
  the player's objective. These belong behind diagnostics, not in the game HUD.
- A large image and multiple permanent sidebars compete with the decision.
- Role slogans do not explain responsibilities, legal choices, voting or scoring.
- The existing public documentation describes the recorded browser fixture and
  technical boundaries. It is not a player rulebook.
- The backend already exposes outcome checks, votes and missing roles; the
  browser adapter discards them and shows only an aggregate score and reason.

Sources: [current forms](../clients/agent-heist-web/src/AgentHeistClientView.tsx),
[adapter](../clients/agent-heist-web/src/liveAdapter.ts), and
[existing demo docs](../web/demos/src/AgentHeistDocsPage.tsx).

## What the current game actually is

Heist 0.5 is a timed cooperative hidden-information **planning** game.
Its schema-safe implementation delegates gameplay through 0.4 and 0.3 to the
clock-safe rules. It does not execute a physical heist in a navigable world.

1. A Navigator can inspect the route. An Insider can inspect the entry time.
   A Broker can inspect equipment and extraction.
2. Players publish true intel or trade it privately. A published false clue is
   rejected; the game is not a supported lying/false-clue mechanic.
3. Players propose a four-part plan, endorse one, or challenge a detail that
   conflicts with intel they know.
4. Each player seals one plan choice and a resource-contribution choice.
5. At least two players must choose the same plan. No strict majority means
   failure and zero points. An endorsement is not a sealed vote.
6. The selected plan earns one point each for correct route, entry time,
   equipment and extraction, plus one if any supporter of that selected plan
   contributed a resource. Five means success; three or four means
   `partial_failure`; zero to two means failure. This is a crew score.

The resource is currently a boolean, not an inventory item with scarcity,
cost or ownership. Do not draw a backpack or invent a cost to explain it.
There are only three coherent hidden fixture tuples today; an interface
redesign alone will not make the game strategically deep.

Current timing after Host start: 30 seconds briefing, 90 seconds negotiation,
30 seconds commitment, automatic resolution, and 20 seconds result (some
phases may finish early under their existing rules). A browser help panel,
tab switch or re-entry never pauses that clock.

Important edge case: an absent Broker removes access to the equipment and
extraction clues. The two remaining players must guess them; the UI must not
quietly reveal or reassign that intel. Whether to change that rule is a
separate Pack revision, not a presentation fix.

Sources: `crates/worldstream-core/src/agent_heist_lobby_v5.rs:28`,
`agent_heist_lobby_v3.rs:177`, and `agent_heist_clock_safe.rs:377`, `:405`,
`:591`, `:663`, `:728`, `:939`, `:1027`, `:1575`.

## Three prototypes

All three run the same local, untimed, scripted practice. No login, Room,
WebSocket, LLM call, provider cost, persistence or leaderboard write occurs.
The sample is the existing service / early / thermal key / boat fixture.
It is deliberately not the live client adapter and must never be copied into
live state as a fallback. Never bundle the hidden fixture bank as live hints.

| Variant | Structure | Strength | Tradeoff |
| --- | --- | --- | --- |
| A — Mission focus | Scene and progress beside one decision surface; compact single column on phone | Clearest first move and guided progression | Needs an easy route back to the shared board in live negotiation |
| B — Crew tabletop | Teammates around a central work surface; your intel is a hand | Shared planning and cooperation are more visible | Denser and less natural on a small phone |
| C — Interactive story | Full-scene backdrop with a narrative prompt and a bottom decision panel | Strongest fiction and sense of a moment | Weaker comparison of simultaneous plans; can become a text adventure |

Open the local study at `http://127.0.0.1:5188/agent-heist-v8/?variant=A`.
Change A to B or C, use the bottom arrows, or use left/right keyboard arrows
outside an input/dialog. Layout changes preserve the current practice state.
`&step=3` opens the plan builder; steps 0–5 cover the complete practice.
**Study controls** exposes sample state and stage jumps. These are not game UI.

On branch `prototype/heist-player-experience`, after the normal dependency
setup, run `npm run heist:prototype` (or `pnpm heist:prototype`). The prototype
is gated by Vite development mode and a `variant` query parameter on the
existing standalone route. The normal route remains unchanged.

## Recommended player journey

### Before joining: a playable tutorial, not a manual

Offer **Try a practice heist** beside **Play live** on the game's entry page.
Use a short scripted round with no account requirement, no timer pressure,
no provider calls and no public score. Label it practice throughout.
Returning players can skip it. Do not force a long tour in an already ticking
live Room. Keep tutorial completion as a device preference only if useful.

Teach by doing: open a dossier, see its private contents, share it, choose
four plan cards, and seal the choice. Then explain the five scoring checks.
Make an incorrect choice possible so the player can see consequences.
The prototype simplifies teammates and auto-advances practice phases explicitly;
the live game will never infer that a button advances the shared phase.

### Waiting room: the short mission briefing

Show the goal, the player's role in one sentence, three steps to play,
the crew roster, start/fill status and countdown. Link to **How to play**.
No full tutorial forced into the 30-second fill window. If practice opens
from an existing waiting room, warn that waiting-room formation keeps moving;
prefer the pre-join practice path.

The platform owns waiting-room formation, not a Heist renderer. A small
reviewed listing field for plain static instructions plus a help/practice
link is sufficient. No Pack-ID branch or embedded executable client in the
generic platform. That listing metadata extension needs its own bounded
contract review; do not build a universal rich-content/CMS system for it.

### Live briefing: an object, not an identifier

Show **Your route dossier — Open** to a Navigator. Clicking it submits the
existing inspect Action for the associated authorized target. Only a committed
authorized update reveals its contents. Use **Only you can see this** visibly.
An Insider and Broker receive their own appropriate objects, never another
role's hidden answer. Missing data means unavailable, not a fixture fallback.

### Live negotiation: decisions on the thing being decided

- **Share with crew** on a known clue; the client carries its exact code.
- **Trade privately** in a secondary sheet, selecting known clues, eligible
  recipients and requested consideration. No typing IDs.
- **Build a plan** with four named option groups and a final review. Show what
  authorized intel supports each option; unknown means unknown.
- **Back this plan** / **Flag a conflict** directly on an existing plan card.
- Incoming trade cards show the exchange plainly and one **Accept trade** action.
- Keep all currently offered advanced actions reachable. A suggested next move
  is guidance, not an enforced linear workflow or an automatic action.

In live play, receive concurrent crew updates without replacing a focused
decision, wiping a draft, or moving buttons. A phase transition can invalidate
the draft: explain that, retain it only as a clearly non-submittable local note,
and show the next valid screen. Do not optimistically claim an Action committed.

### Commitment and results

Pick a named plan card, review the four choices, make the contribution choice,
and choose **Seal my choice**. Say clearly that this is not an endorsement and
cannot be revised under current rules. Display the shared deadline and receipt
status separately from acceptance in Room state.

The debrief shows the crew result, whether a plan obtained a majority, missing
votes when authorized, and the five true/false checks. If there was no majority,
show that reason instead of inventing a five-check breakdown. A check failure
does not authorize revealing the correct hidden answer. Only show an answer
if it was already known/public or a separately authorized final reveal provides
it. The practice may reveal its scripted answer; the live client cannot.

Offer **Play again** and **Back to games** through the existing platform flow.
Do not recompute leaderboard truth in the client. Public result publication
can lag the live debrief; show that distinction without technical jargon.

## A thin client can be a rich game

ADR 0017 explicitly says clients are **protocol-thin and experience-rich**.
This work belongs in the independent Heist Activity Client, not WorldStream
Inspector, the platform's generic room controls or the kernel.

| Layer | Minimum work |
| --- | --- |
| Existing Heist client | Role-specific copy, game objects, focus/navigation, guide, drafts, dialogs, receipt states and existing session/reconnect behavior |
| Client adapter | Preserve and validate already-authorized outcome `checks`, `vote_counts`, `missing_roles`; do not discard them |
| Pack-authorized choices | Expose safe targets for unopened clue objects and other dynamic choices where current offers/projection do not supply them |
| Platform | Reviewed instructions/help links, existing setup/start/exit/re-entry and results brokerage; no Heist gameplay implementation |
| Runtime | No game-specific changes proposed |

Current type-level Action Offers do not guarantee that an arbitrary target is
legal. A static exact-revision label map can turn `entry_window` into
**When to enter** and schema enum options into readable choices. Dynamic legal
targets, visibility, currently available trades and hidden facts must come
from Pack-authorized data; client inference must not become a second rules engine.
Use a narrow Pack successor only where this data is genuinely missing; inspect
the existing schemas/projection first. Do not add a generic UI interpreter.

Game Actions still go through the same scoped SDK/session and current
offer/schema/Room-sequence checks. WebMCP and agent clients remain peers;
the human tutorial does not become an authority or special agent tool.

## Delivery after the design decision

1. Watch three to five people who have never used WorldStream play the local
   practice. Do not explain it for them. Select the layout based on confusion
   and successful choices, not screenshots alone.
2. Freeze the human interaction contract and minimal authorized choice data.
   Keep rules unchanged unless a distinct Pack change is explicitly accepted.
3. Implement a vertical live slice: enter → open own dossier → receive private
   intel → share → see the shared update. Verify all roles and spectator privacy.
4. Add plan/trade/commit flows and the real server-supplied debrief. Add waiting
   room instructions and a public practice entry point using reviewed metadata.
5. Verify full live gameplay, phase changes during edits, rejected actions,
   reconnect, expired clocks, absent Broker, exit/re-entry and result publication.
   Release a new immutable client/listing; preserve existing pinned releases.

Acceptance targets (proposed, not measured yet): new players explain the goal
and team score; make their first valid move within 20 seconds after the brief;
complete the main flow without raw-ID entry or facilitator help; know which
intel is private; and can explain a failed outcome.

Primary controls should fit normal desktop and phone viewports without page
scrolling to find an action. This is not a ban on accessible scrolling:
long rules/history use a focused sheet, and small screens or enlarged text
must reflow rather than clipping content. Keep touch targets, keyboard focus,
screen-reader labels, reduced motion and explicit text alongside art.

## Current limits and open decisions

The prototype covers the Navigator's guided main path only. Private trading,
multiple simultaneous plans, live timing, multiple roles, spectator rendering,
reconnect/re-entry and network failures are requirements for the live redesign,
not working capabilities of this prototype. Its sample teammates are not LLMs.
No production-ready implementation or user playtest success is claimed.

Local runnable checks: TypeScript compilation passed. The Navigator practice
was clicked through from dossier to share to all four plan choices and a sealed
vote, producing the expected sample 5/5 debrief. The three desktop layouts were
rendered at 1440×900. Mission focus's plan decision was checked at 390×844:
no page overflow, with its primary action fully visible. These are prototype
checks, not live acceptance, an accessibility audit or novice playtest evidence.

The existing [IMO-214](https://linear.app/imom39a/issue/IMO-214/apply-proven-archive-mission-guidance-to-agent-heist)
is blocked by Archive player evidence (IMO-213). This direct Heist study is
additional design evidence; it neither closes that ticket nor silently removes
its blocker. Before production work, explicitly decide whether this urgent
Heist usability slice should become a separate ticket ahead of Archive.

Two frontier decisions for the user:

1. Keep the current planning rules for this UX rescue, or design new physical
   gameplay now? Recommend the former: make this game comprehensible first.
2. Use Mission focus as the primary direction, adding the crew board on demand,
   or prefer Tabletop / Interactive story? Recommend Mission focus plus board.

Tutorial before joining is the recommended onboarding approach for this
proposal. Longer timers, pause, resource costs, deception, map movement and
new scoring are separate gameplay design decisions, not quietly bundled here.

## Follow-up: illustrated cards and the purpose of choosing

The user likes the practice direction and requests original artwork for the
service entrance, thermal key, boat and other game objects. Keep all three
layouts. Replace the Roman-numeral choice placeholders with consistently
illustrated route, equipment and extraction cards. Keep readable HTML labels,
selection state, keyboard support and compact phone controls. Illustrate all
alternatives with equal care; art must not identify which answer is correct.
Reuse the same object artwork in the dossier and plan review for recognition.
Entry-time choices remain a clear early/middle/late window visualization, not
invented clock times or a night-to-day mechanic.

This does not make Heist a prisoner's dilemma. The current rules have a common
crew score and no individual payoff for betrayal. A prisoner's dilemma needs
an incentive structure in which individual defection is tempting even though
mutual cooperation is better than mutual defection. Simply making votes
private does not create that structure.

There are two separate decisions in today's game:

1. **Plan construction:** choose a route, entry window, tool and extraction.
   These are a proposal for a whole operation, not four independent crew votes.
2. **Crew commitment:** pick an existing complete plan. At least two players
   must commit to the same plan; unanimity is not required. Only then does
   the server score that plan against the hidden facts and contribution rule.

Example: Navigator knows service entrance; Insider knows early; Broker knows
thermal key and boat. Combining them gives a correct proposal. Two players
must still commit to that proposal, and a supporter must contribute for 5/5.
Two votes for a wrong plan agree but lose correctness points. Three different
plan votes have no majority and earn zero.

The prototype intentionally scripts teammate disclosure and agreement so the
first practice teaches controls. It does not currently test the human's
negotiation or coordination skill. The real game's interesting promise is
pooling different information and coordinating under a shared deadline.
But the present rule set is shallow after all truthful clues are shared,
and contributing has no personal cost. Art and better wording cannot fix
that lack of tradeoffs. Do not market this as a deep strategic benchmark.

Recommended later gameplay direction, NOT implemented or frozen: a cooperative
constraint puzzle with meaningful tradeoffs (for example a limited number of
inspections and several viable plans with differing risks). An alternative is
a mixed-motive social game with explicit personal/team payoffs. These are
different design goals. Adding a contribution cost alone does not automatically
make either one a prisoner's dilemma; specify and test the payoff structure.
