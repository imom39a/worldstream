# Agent Heist commitment receipt successor

Agent Heist client v12 is an immutable client-only successor. It leaves Pack
`worldstream.agent-heist@0.5.0`, Room configuration, deadlines, role ownership,
House Agents, Result Projector, and publication policy unchanged.

The client now renders a participant-private “Your choice is sealed.” receipt
from the authorized Projection's `ownCommitment` field. It does not reveal
another participant's choice. The rendered hosted journey submits the human
Navigator action immediately, observes that private receipt independently from
the public crew count, and keeps stale-action reconnect recovery inside the
Pack's original absolute 30-second action deadline.

## Immutable identities

- Build tree: `sha256:904712cf839e686d214961ac9fc4109d8e74289b28567c174164c82ee60507c1`.
- Release claims: `sha256:cb027c265a7893173f144f225968b6ce4359ec8ce4b1151424c44e4bc51f4b4c`.
- Conformance evidence: `sha256:9dacabab5d29d5a74fcdb38d98e560d3eef745c984c84f096ad0312fbaf8ff14`.
- Release: `sha256:02d8cb123ba2c5b4c9b4e6b075a7eb21ce3f551cc53332e836b1690b2b4ef519`.
- Listing 0.29.0: `blake3:71eb289ce6c730fa6e8eefc9d222f9730e8a69b6cc297d43b8794b7eda35f5f9`.

Entrypoints are `/agent-heist-v12/`, `/agent-heist-v12/hosted/`, and
`/agent-heist-v12/practice/`.

The v11 build was reproduced from clean source and frozen under
`config/activity-clients/artifacts/agent-heist-web-v11/`. Its observed tree
digest is the published
`sha256:ce7c86d24832158536df90384d140ae5aec6482eb654ab70c1551effc9e62093`.
Listing 0.28.0 continues to resolve to `/agent-heist-v11/hosted/`; no historical
release, Listing, binding, or route bytes changed.

## Local qualification

The release verifier checked the exact artifact, claims, evidence, release,
deployment, binding, and mount references. Focused runtime, catalog overlay,
workspace, Vercel packaging, hosted development, platform, demos, and built
browser tests passed. The browser check covers current v12 and retained v11
routes. These results are local conformance evidence; deployment and production
acceptance remain separate coordinated actions.
