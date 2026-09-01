---
status: accepted
date: 2026-09-01
---

# Separate Activity Clients from Packs and Studio

## Context

WorldStream already keeps `worldstreamd` authoritative and Activity Pack
execution deterministic, but the first web implementation combined three
different products in one Participant Console: recorded Agent Heist fixture
inspection, Pack-neutral protocol inspection, and Pack-specific participant
rendering. Studio then launched that one Console regardless of the Room's exact
Activity Pack Revision.

That packaging makes a polished Pack experience look like a generic JSON
debugger, encourages Pack-ID branches in one centrally maintained frontend,
and creates pressure either to put executable presentation code in `.wspack`
or to make Studio a Participant client. Both choices weaken accepted Pack and
Studio boundaries.

## Decision

An **Activity Client** is an independently executing application that uses a
scoped WorldStream client contract for one or more exact Activity Pack
Revisions. Browser applications, CLI/TUI programs, mobile applications,
Python programs, and agent-owned clients are protocol peers. An Activity Pack
may have zero, one, or many Activity Clients, and an Activity Client may support
more than one exact Pack revision after explicit compatibility testing.

An Activity Pack Bundle remains headless. It carries deterministic rules,
schemas, codecs, immutable static material, and conformance evidence, but no
HTML, browser JavaScript, mutable launch URL, or frontend authority. Client
selection and deployment are operational integration state. They never enter
Genesis, Authoritative Room State, Transitions, canonical hashes, or Replay.

Studio remains the Host Operator control plane. It may discover and open an
approved compatible Activity Client for a provisioned Membership, but it does
not import Pack-specific participant renderers, impersonate another Room
Member, or receive participant-private Projection data merely to launch a
client. The user-facing action is **Open participant client**, not “Open
Participant View.”

The first-party **WorldStream Inspector** is the Pack-neutral browser fallback.
It displays the exact authorized Projection/Observation data, Action Offers,
receipts, connection state, and Replay evidence supplied by the protocol. It
does not infer a Pack-specific experience or maintain a Pack renderer registry.

The first implementation proves this boundary without freezing a third-party
client ABI:

- one trusted local Client Host serves the Inspector and first-party Agent
  Heist client on separate paths under one exact approved origin;
- the Supervisor selects the Heist path only for an explicitly supported exact
  Agent Heist revision digest and otherwise selects the Inspector;
- the existing one-use fragment handoff and retained HttpOnly
  Membership-scoped session remain the launch authority seam;
- Agent Heist recorded and live modes share presentation modules but use
  disjoint adapters; and
- live mode starts without Activity data, installs only an authorized
  Projection Reset/Observation, replaces omitted state instead of falling back
  to fixture data, and exposes no client-side Projection-lens switch.

The existing `participant-console` route names, wire versions, and release
artifact names remain compatibility names during this migration. They do not
change the domain boundary established here.

## Consequences

- Pack Authors can define rules without learning a frontend ABI. Application
  Integrators can build domain-specific clients in any language that implements
  the ordinary client contract.
- A UI-only release does not create a new Activity Pack Revision, and a new
  Pack revision is not assumed compatible with an older client by name or
  semantic version.
- Studio and the Inspector no longer accumulate new Pack-specific participant
  branches. Existing Negotiate coupling is migration debt and moves to its own
  Activity Client after the Agent Heist proof.
- A missing or incompatible specialized client fails closed to the Inspector;
  it does not prevent SDK, CLI, or agent participation.
- The local single-origin Client Host is a first-party release seam, not a
  general trust model for third-party executable code.
- A signed/content-addressed client bundle, local client registry, network
  catalog, automatic installation, multi-origin handoff, sandboxed embedding,
  and marketplace remain deferred. Each requires a separate contract and
  security review.

This decision refines ADR 0013's Studio/Participant Console distinction and
the presentation boundary in ADR 0014. It does not change the Room Kernel,
ActivityPackV1, protocol authority, storage, or Replay semantics.
