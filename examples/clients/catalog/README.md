# Client contract fixtures

This catalog retains immutable release, distribution, and binding declarations
used by Rust and JavaScript contract tests. The declarations also illustrate
how a Host approves an exact client build for a Pack revision and access mode.
Historical compiled browser assets and the hosted deployment are removed.

The source-built examples are Heist v12, Negotiate v3, Midnight Archive v14, and
Inspector v2. Build and serve them with `pnpm activity-clients:serve` from the
repository root. See [browser clients](../README.md).

Do not import historical declarations as proof of the bytes you just built.
`scripts/prepare-local-activity-client.mjs` computes new release identities and
prepares declarations for review. Existing approvals belong to the local Host;
a source checkout does not rewrite them. See [Activity Clients](../../../docs/activity-clients.md).
