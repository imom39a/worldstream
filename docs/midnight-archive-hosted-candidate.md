# Midnight Archive internal hosted candidate

IMO-207 established the first local hosted candidate. IMO-211 advances it with
a new immutable revision under the retained Listing ID without publishing it.
The current reviewed revision is `worldstream.midnight-archive.internal-solo` version
`0.2.0`, with digest
`blake3:efa4b63c9da251431343fbaab03e85ee2de053722345f9bd7b1434c2762283f3`.
Version `0.1.0` remains retained for exact old-Run resolution.

This is an intermediate, unlisted solo qualification revision. It is not the
four-option first Archive release specified by ADR 0027. IMO-209 must publish a
new immutable Listing revision with solo, Mira, Jonah, and full-crew options
plus their exact reviewed House Agent identities before that release exists.

The current Listing pins:

- Pack Revision
  `blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07`
  in Bundle
  `blake3:de3cd1d9fa45087b69cb107a663596305864c350f620fe4d7260e0341214d47a`.
- Activity Client v12 Release
  `sha256:fae51aaa770810d203f00fd8be7d098b069ce2ef8e6fc9b7f31b80accf53345f`
  with artifact
  `sha256:c3c200e2ecb55c53bac24ddb31153fb667be1f64999d6ab00a601dd2ad670065`.
- Result Projector 0.2.0
  `blake3:93bdc21b4b09ec6e7c1ed7a11df80d984e2f80175e9fae01143f3a00c65d4a17`
  and Public Projection v5
  `blake3:c8045ca0762df97d4e82488f562f7a69eea2b480e41f06889eef3dff133086d0`.

The only current seat is the human expedition lead. Configuration is fixed to
`standard-v1`, launch input is empty, and no House Agent revision is allowed by
this Listing. Anonymous viewing and result publication are disabled.

Before launch, the Listing discloses that an active expedition expires 24 hours
after the Host records Activity Start. Closing a tab, disconnecting, or
re-entering does not pause or reset that deadline. The deadline applies only
while the Pack is nonterminal. The recorded Pack timer moves an active Room to
the terminal `expired` phase with a null Outcome. It preserves discoveries,
resources, completed specialist work, and the fixed deadline while recording no
crew extraction or left-behind result. Every terminal path cancels outstanding
session and companion-response timers; stale timers and late replies are inert.

The 0.2.0 Result Projector uses the minimal authorized Public Projection and
classifies `lifecycle: terminal`. An expired Projection therefore records
`terminal_without_outcome`. Existing terminal retirement releases the Run's
active-capacity reservation once while retaining the Run, Membership identity,
and consumed allowance. It does not create an Indexed Activity Result or
suspend a resumable Runner. My Games can reopen the retained private terminal
Room through the original Membership correspondence.

The BFF's authenticated `/api/catalog/internal` route includes this nonpublic
Listing only when its exact digest appears in
`internalCandidateListingDigests`. Its launch button remains unavailable unless
`hostedActivityAvailable` verifies the exact approved Pack, client Release,
surface, Client Binding, running inventory, and served artifact bytes. Missing,
pending, mismatched, or unhealthy dependencies fail closed. Production
construction does not opt into this internal candidate.

`pnpm hosted:dev` reads `config/hosted/internal-candidates.json`, approves and
installs the exact Bundle, imports the exact hosted client, and configures the
local availability check. An empty candidate list still starts the public-only
library. Retained Listing 0.1.0 resolves with its original v10 client and
Projector 0.1.0, but it is not the current discovery entry.

Qualification evidence is recorded in
`packs/midnight-archive/evidence/qualification-imo-211.md`. The evidence uses a
real Component Host, SQLite Room, WebSocket streams, injected Host clock,
recorded scheduler, exact Replay, hosted projector runtime, local Postgres RPCs,
and the durable Host allowance ledger. These are composed boundary proofs; no
24-hour wall-clock wait or deployed production run was performed.

Run `node scripts/verify-midnight-archive-hosted.mjs` against `pnpm hosted:dev`
for the current signed-in solo launch/play/debrief journey. The script targets
the exact v12 hosted client. Historical IMO-207 journey artifacts remain under
`.worldstream/evidence/imo-207`; new runs write under
`.worldstream/evidence/imo-211`.
