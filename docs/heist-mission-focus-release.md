# Agent Heist: Mission focus release

## Scope

The user selected **A / Mission focus**: an illustrated, guided experience for
the existing Heist game. New multi-stage gameplay is a separate project.

This release changes the Activity Client, not the kernel or game rules. The
current Heist Pack stays at `0.5.0`. Scoring, deadlines, role ownership, legal
actions, House Agent revisions, and spending limits stay unchanged.

## Player experience

- A focused current move replaces the long action-form sidebar.
- Players choose named dossiers and plans, not internal IDs.
- The plan builder shows one decision at a time: way in, entry window,
  equipment, and way out. Each choice has its own illustration.
- The shared board, private intel, and rules are available when needed.
- The live clock remains a countdown. Opening help does not pause a live game.
- The debrief explains the five crew checks from the server result. This is
  not a model benchmark or an individual skill ranking.
- Separate guided practice is scripted and untimed. It needs no account,
  live Room, API key, or paid model call. Practice scores are not published.

## Authorization boundary

Live state starts empty and installs only an authorized Projection Reset.
Practice state is never copied into a live session.

Heist `0.5.0` publishes a fixed role-to-object mapping in its immutable rules:
Navigator has the route dossier; Insider has the entry-window dossier; Broker
has equipment and extraction dossiers. The client uses a reviewed descriptor
keyed to that exact Pack digest to label these objects. It contains **no clue
answers**. It is not dynamic target metadata returned by the server. Unknown
revisions have no inferred dossier targets.

The current inspection offer, authorized role, and already opened private
clues control the inspection affordance. Submitted moves use the current offer
and sequence; the server decides legality. Public spectators receive no
private intel or participant moves. Local drafts survive unrelated crew
updates but do not become Room truth before the server accepts an action.

## Release strategy

Publish immutable Client v9 paths at `/agent-heist-v9/`,
`/agent-heist-v9/hosted/`, and `/agent-heist-v9/practice/`. Listing `0.26.0`
selects the new hosted client for new Rooms. Retain Listing `0.25.0` and its
original v8 artifact bytes for existing Rooms. Keep all earlier supported
client paths and retained result-projector mappings.

Append the reviewed v9 declarations and Listing allowlist entry to the current
Fly installation. Do not reset its database, replace its image, rebind existing
Rooms, change provider approvals, or add Machines for this UI release.
The Supabase migration adds immutable catalog metadata only. It changes no
schema, RLS policy, operating gate, or authentication setting.

## Evidence and status

Tracking: [IMO-215](https://linear.app/imom39a/issue/IMO-215).

Implementation, identity generation, and release verification are in progress.
This document does not yet claim a successful production deployment or a paid
live match. Record exact identities and observed checks here before handoff.

Artwork provenance and prompts are in
`clients/agent-heist-web/src/prototype-player/art/README.md` and
`entry-window-prompts.json`. Production uses bounded WebP derivatives.
