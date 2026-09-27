# Negotiate browser client

This independently packaged Activity Client demonstrates typed negotiation
offers, approvals, and scoped participant views for the supported exact
Negotiate Pack revisions. It starts empty and installs the authorized Projection
Reset delivered through a retained session.

From the repository root:

```sh
pnpm --dir examples/clients/negotiate-web test
pnpm --dir examples/clients/negotiate-web build
pnpm activity-clients:serve
```

The current source build is served at `/negotiate-v3/`. Historical compiled
builds are removed. Live participation needs a local Controller and an authorized
handoff; see [browser client setup](../README.md).
