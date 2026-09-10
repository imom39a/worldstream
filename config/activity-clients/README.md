# First-party client design release

Current source builds are Agent Heist v6, Negotiate v3, Midnight Archive v9,
and Inspector v2. Each release has an exact build-tree digest and linked
evidence under `releases/` and `conformance/`. The shared CSS in `web/design/`
is bundled independently; it does not create a runtime dependency between
clients.

| Current path | Source |
| --- | --- |
| `/agent-heist-v6/` and `/agent-heist-v6/hosted/` | `clients/agent-heist-web` |
| `/negotiate-v3/` | `clients/negotiate-web` |
| `/midnight-archive-v9/` and `/midnight-archive-v9/hosted/` | `clients/midnight-archive-web` |
| `/inspector-v2/` | `web/console` |

The prior Heist v2, Negotiate v2, and Inspector v1 build trees under
`artifacts/` were reproduced from commit `ad71a1c` and verified against their
unchanged release digests before retention. Midnight Archive v1 through v8 and
Negotiate v1 remain retained too. Midnight Archive v7 and v8 are retained,
superseded, unqualified Pack-candidate client releases; v9 is the current
production-proved revision. Do not add files inside a retained build tree
or modify its bytes. Inspector v1 used root-relative assets, which the local Host
continues to serve for that release.

`cli-import.json` proposes the exact releases and current local bindings for
operator approval. Existing installed approvals are not rewritten by a source
checkout. `hosted-local-import.json` proposes the new hosted release; deployment
startup preserves an existing installation’s Inspector fallback when present.
Retained and current Midnight Archive bindings can all remain `default` because
selection is scoped to the Room's exact Pack revision digest. The repository
binding-store test proves the nine Pack digests resolve v1 through v9 even
though all revisions retain the same publisher version label.

The hosted platform installs the current Heist v6 build and retained v5/v4/v3/v2
artifacts. Listing 0.14.0 and its database migration pin v6, while retained
Listing 0.13.0 resolves v5. Publish through the coordinated hosted release process; changing
frontend files alone does not activate a Runtime or database migration.

Run `pnpm activity-clients:build`, `node scripts/verify-activity-clients.mjs`, and
`node --test tests/activity_client_workspace.test.mjs` to check current bytes,
retained mounts, and exact binding references. The full conformance lane remains
`pnpm activity-clients:verify`.
