# IMO-233 hosted version-binding evidence

This evidence closes the hosted deployment-identity gap at clean source commit
`ece028e1c9ad81ac43b8c05289a93d115a957444`. The preflight and runbook fixes
are implemented by `ef3147ce` and `ece028e1`.

## Local preflight and tests

`node --test scripts/hosted-deploy-preflight.test.mjs` passed both tests. The
cases exercise the documented command construction plus missing source,
missing Docker build identity, missing runtime identity, malformed identity,
mismatched identity, and dirty-checkout rejection. Running the preflight from
the dirty primary checkout exited non-zero before publication or Fly Machine
mutation. The successful clean-worktree plan is
[`deployment-plan.json`](deployment-plan.json), SHA-256
`404064768b100a9b068f387c6912375161aa94eb8719911d978a2eb1103d5e66`.

The plan binds all three identities to the same immutable value:

* clean source revision:
  `ece028e1c9ad81ac43b8c05289a93d115a957444`
* Docker `SOURCE_REVISION` build argument:
  `ece028e1c9ad81ac43b8c05289a93d115a957444`
* runtime `WORLDSTREAM_DEPLOYMENT_VERSION`:
  `ece028e1c9ad81ac43b8c05289a93d115a957444`

The deployment identity is a plain environment value. The runbook provisions
the four runtime secrets separately and does not infer the deployment identity
from image tags or mutable state.

## Fresh Fly boot

The exact generated command was run against the fresh disposable Fly app
`worldstream-imo233-firstboot-20260914`. Its first and only Machine,
`8d96934c322168` in `iad`, started successfully on its initial launch. Before
the explicit restart, its event history contained only the pending and created
launch events followed by a successful start; there was no exit event. The
readiness service check passed. The captured state is
[`machine-first-boot.json`](machine-first-boot.json), SHA-256
`ad41149d59aecf847ffd9dfd39a18065ea605ff08769e7d5c9aef36939a007bc`.

The Machine used the smallest verified hosted profile: one shared CPU, 1 GiB
RAM, and a 1 GiB encrypted persistent volume. The published image digest was
`sha256:2ad5c01aaf510e308ffd8cfa84958db99a5f1724afc912d097b5a3e461edbf93`.
Its OCI `org.opencontainers.image.revision` label exactly matched the clean
source commit.

The live endpoints returned:

* [`healthz.json`](healthz.json):
  `{"status":"ok","version":"hosted_gateway_health.v1"}`
* [`readyz.json`](readyz.json):
  `{"status":"ready","version":"hosted_gateway_readiness.v1"}`
* [`version.json`](version.json):
  `{"deployment":"ece028e1c9ad81ac43b8c05289a93d115a957444","version":"hosted_gateway_deployment.v1"}`

## Same-image restart

An explicit restart stopped and started the same Machine. Its readiness check
returned to passing, the three endpoints returned the values above, the image
digest was unchanged, and both the OCI revision label and `/version`
deployment identity still matched the tested commit. The captured restart
state is [`machine-after-restart.json`](machine-after-restart.json), SHA-256
`761e1f3a1c7ce5bc4de11271b1a4a1fd70f58fb58646d31430cb4d46c7a686ae`.

The app and its companion disposable validation app were destroyed after the
evidence was captured.
