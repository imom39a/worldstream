# Browser clients

Independent Activity Clients demonstrate the scoped WorldStream client contract:

- [Agent Heist](agent-heist-web/README.md): hidden-information game views.
- [Negotiate](negotiate-web/README.md): typed offers and approval views.
- [Midnight Archive](midnight-archive-web/README.md): cooperative game views.
- [Inspector](inspector/): Pack-neutral projections, observations, and receipts.

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm activity-clients:check
pnpm activity-clients:serve
```

The loopback client host serves current builds at `/agent-heist-v12/`,
`/negotiate-v3/`, `/midnight-archive-v14/`, and `/inspector-v2/` on port 5173.
Live participation also needs a running local Controller, Room Membership, and
an authorized handoff; opening a page alone does not join a Room.

`catalog/` retains immutable release declarations used by contract tests and as
templates for `scripts/prepare-local-activity-client.mjs`. They are not approvals
for a rebuilt client. Historical compiled browser assets have been removed.
Generate and review declarations for the exact bytes of a local build before
importing it. See [Activity Clients](../../docs/activity-clients.md).
