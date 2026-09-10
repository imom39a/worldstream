# First-party client design release

Current source builds are Agent Heist v9, Negotiate v3, and Inspector v2. Each
release has an exact build-tree digest and linked evidence under `releases/`
and `conformance/`. The shared CSS in `web/design/` is bundled independently;
it does not create a runtime dependency between clients.

| Current path | Source |
| --- | --- |
| `/agent-heist-v9/`, `/agent-heist-v9/hosted/`, and `/agent-heist-v9/practice/` | `clients/agent-heist-web` |
| `/negotiate-v3/` | `clients/negotiate-web` |
| `/inspector-v2/` | `web/console` |

The prior Heist v2, Negotiate v2, and Inspector v1 build trees under `artifacts/`
were reproduced from commit `ad71a1c` and verified against their unchanged
release digests before retention. Negotiate v1 remains retained too. Do not add
files inside a retained build tree or modify its bytes. Inspector v1 used
root-relative assets, which the local Host continues to serve for that release.

`cli-import.json` proposes the exact releases and current local bindings for
operator approval. Existing installed approvals are not rewritten by a source
checkout. `hosted-local-import.json` proposes the new hosted release; deployment
startup preserves an existing installation’s Inspector fallback when present.

The hosted platform installs the current Heist v9 build and retained v8/v7/v6/v5/v4/v3/v2
artifacts. Listing 0.26.0 and its database migration pin v9. Retained
Listing 0.25.0 resolves v8, while retained
Listing 0.24.0 resolves v7. Publish through the coordinated hosted release process; changing
frontend files alone does not activate a Runtime or database migration.

Run `pnpm activity-clients:build`, `node scripts/verify-activity-clients.mjs`, and
`node --test tests/activity_client_workspace.test.mjs` to check current bytes,
retained mounts, and exact binding references. The full conformance lane remains
`pnpm activity-clients:verify`.
