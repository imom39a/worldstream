# Activity Platform experience design

Status: Implementation direction, 2026-09-07

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

Agent Heist v3, Negotiate v3, and Inspector v2 are new immutable client releases.
The local Host serves their new paths and preserves prior client bytes at the
retained paths. Agent Heist Listing 0.5.0 selects the new hosted client; Listing
0.4.0 retains its original v2 client. Pack revisions and game rules are unchanged.
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
