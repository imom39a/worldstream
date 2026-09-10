# First-party client design release

Current source builds are Agent Heist v10, Negotiate v3, Midnight Archive v13,
and Inspector v2. Each release has an exact build-tree digest and linked
evidence under `releases/` and `conformance/`. The shared CSS in `web/design/`
is bundled independently; it does not create a runtime dependency between
clients.

| Current path | Source |
| --- | --- |
| `/agent-heist-v10/`, `/agent-heist-v10/hosted/`, and `/agent-heist-v10/practice/` | `clients/agent-heist-web` |
| `/negotiate-v3/` | `clients/negotiate-web` |
| `/midnight-archive-v13/` and `/midnight-archive-v13/hosted/` | `clients/midnight-archive-web` |
| `/inspector-v2/` | `web/console` |

The prior Heist v2, Negotiate v2, and Inspector v1 build trees under
`artifacts/` were reproduced from commit `ad71a1c` and verified against their
unchanged release digests before retention. Midnight Archive v1 through v12 and
Negotiate v1 remain retained too. Midnight Archive v7 and v8 are retained,
superseded, unqualified Pack-candidate client releases; v9 is the retained
unavailable-companion revision, v10 through v12 are retained authored-scenario
builds, and v13 is the current four-roster build. Do not add files inside a
retained build tree or modify its bytes. Inspector v1 used root-relative assets,
which the local Host continues to serve for that release.

`cli-import.json` proposes the exact releases and current local bindings for
operator approval. Existing installed approvals are not rewritten by a source
checkout. `hosted-local-import.json` proposes the new hosted release; deployment
startup preserves an existing installation’s Inspector fallback when present.
Retained and current Midnight Archive bindings can all remain `default` because
selection is scoped to the Room's exact Pack revision digest. The repository
binding-store test proves the ten Pack digests resolve v1 through v10 even
though all revisions retain the same publisher version label.

The hosted platform installs the current Heist v10 build and retained v9/v8/v7/v6/v5/v4/v3/v2
artifacts. Midnight Archive v1 through v12 remain retained, with v13 bound only
to the four-roster Pack Revision. Listing 0.26.0 and its database migration pin
v9, while retained Listing 0.25.0 resolves v8 and Listing 0.24.0 resolves v7.
The session-recovery successor is Listing 0.27.0 and v10. It uses a data-only
catalog seed, not a schema migration. Existing v9 build bytes were checked
against their published digest before retention. See
[the activation gate](../../docs/room-service-reliability.md#successor-and-deployment-gate);
prepared source identities do not prove production activation.
Publish through the coordinated hosted release process; changing
frontend files alone does not activate a Runtime or database migration.

Run `pnpm activity-clients:build`, `node scripts/verify-activity-clients.mjs`, and
`node --test tests/activity_client_workspace.test.mjs` to check current bytes,
retained mounts, and exact binding references. The full conformance lane remains
`pnpm activity-clients:verify`.
