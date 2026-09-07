# Hosted preview status — 2026-09-07

This is a dated deployment record, not a release certificate. The source
identities below describe the observed checkpoint. Verify later deployments
separately through `/api/deployment`; do not infer their identity from this file.

## Current result

The [activity site](https://worldstream-demos.vercel.app) is deployed. Its
public catalog and platform HTTP endpoints pass the checks below. The live
game is **not open for use**. The Fly Machine is stopped and new launches
are closed. No real OpenRouter call has been made during this acceptance
attempt.

Do not infer that gameplay works from a catalog card marked `LIVE`. That
label describes a reviewed activity, not current authority availability.

## Verified deployment

| Item | Observed value |
| --- | --- |
| Activity source | `18bc58e8a8008fc3f2b20a86250468785cb99c05` |
| Vercel deployment | `dpl_2ovkDn3ftW1E2mvv3hVgEQ3nvXC6` — production, Ready |
| Applied Supabase migration head | `20260907164956` — 13 migrations |
| Configured realtime authority | `https://worldstream-preview.fly.dev` |
| Listing | `blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80` |
| Activity Client Release | `sha256:493260d162be2bdfda28e71b6e4f94d5ef5c11e7f03adfbee3ab9295bdf4594b` |

The source is committed and pushed to `main`. Concurrent, uncommitted UI
work is not part of this verified deployment.

Public checks against the production domain passed:

- `/`, `/join`, and a formation deep link return `200`.
- `/api/catalog` and `/api/results/agent-heist/recent` return `200`.
- Anonymous `/api/auth/session` returns the expected `401 session_required`.
- `/api/deployment` returns the exact source, schema, and release identities.
- `/agent-heist-v2/hosted/` returns `200`.
- The retired `/agent-heist/` path and `/api/ws` return `404`.
- The Content Security Policy permits the configured direct Fly WebSocket.

Chrome displays the catalog and its room-setup form. The GitHub sign-in
flow reaches the first-time `WorldStream Preview` consent page. Successful
OAuth callback, account creation, and authenticated session use are not yet
verified; the account owner must complete consent.

## Changes made for deployment

The local Heist client and hosted Heist client have separate entrypoints
in one reviewed release. The hosted client starts empty and installs only
authorized state. Studio/Controller UI does not supply game presentation.
Historical Listing and Client Release records remain unchanged.

The Vercel rewrite now uses an unnamed capture. The named capture previously
added a synthetic `path` query parameter and broke exact API routing. The
Vercel adapter also discards the unused, untrusted standard `Forwarded`
header after its existing proxy-envelope checks. It does not use that header
as authority. Origin, CSRF, and unknown forwarding-header checks remain.

The container validates bootstrap material before it writes state and sets
private volume permissions before it drops privileges. Linux test fixtures
now use test-owned database parent directories; production filesystem safety
checks were not relaxed.

The House approval helper records exact installed profiles, credentials,
Runner Templates, and executable checksums. Registration and activation are
separate operations. The recovery helper can verify a paired, empty initial
installation. It does not certify recovery of populated Room history.

## Cost and safety state

- Vercel remains on Hobby. Supabase remains on Free. No paid upgrade or
  add-on was enabled.
- Fly has one shared-CPU, 512 MB Machine and one encrypted 1 GB volume. The
  Machine is stopped and automatic start is disabled. No additional Machine
  or paid dedicated IPv4 address was created.
- Fly still bills storage while stopped. It has no hard spending cap. Do
  not describe this setup as zero-cost or as protected by a total bill cap.
- OpenRouter Auto Top-Up is off. No credits were purchased by the agent.
- The earlier acceptance key was exposed and must be revoked. Do not use
  that key or an unrelated unlimited key for testing.
- The replacement acceptance key must have a USD 2 lifetime limit, no limit
  reset, and a short expiry. Keep it out of chat, Git, screenshots, and logs.
- Retained House limits also apply: USD 2/day, USD 10/month, and at most ten
  model calls per Assignment. These do not cap Fly, Vercel, or Supabase costs.

## Test evidence and limits

The focused Vercel regression checks passed: 82 Platform tests, one existing
live-test skip, and 19 hosted development tests. The deployed identity verifier
also passes its mismatch-rejection test.

Linux CI run `34151571106` passed the Supabase boundary, isolated gateway,
retained hosted setup, BFF, activity clients/product shell, and local
fail-closed checks. The runner then ran out of disk space in the final Fly
appliance/restart step. The dependent canonical local candidate did not run.
The workflow needs consistent non-incremental, debug-free CI build profiles;
this changes build storage use, not test coverage or production safety checks.

The earlier full Mac workspace run was stopped after its completed suites
passed. Newly launched binaries repeatedly spent a long time before program
entry. That run was built before the final fixture changes and is incomplete;
it is not exact-source release evidence. The stronger standalone browser
proof and full final candidate verification must still complete.

## Remaining work, in order

1. Complete the GitHub consent screen and verify the real authenticated
   browser session.
2. Supply the replacement limited OpenRouter key through an owner-only
   local file. Verify its limits and required provider privacy settings.
3. Finish exact-source CI and local verification. A fake-provider result is
   useful integration evidence, not a real-provider pass.
4. Build and deploy the matching Fly image with `--ha=false`. Keep launches
   closed during setup. The stopped Machine still has the older `2b1d604`
   image; the public site passing does not establish source correspondence.
5. Preserve the existing paired initial-installation backup before any
   credential or installation change. Never edit the retained credential
   vault in place. A fresh initial installation is allowed only after proving
   zero activity and preserving the old pair. This is not a generic upgrade
   or recovery procedure for a used installation.
6. Verify a new paired prelaunch recovery checkpoint against the final image
   and current schema. The earlier successful zero-history drill used an
   older image and schema and does not qualify the new deployment.
7. Register and activate exact House approvals after the deployment,
   credential, privacy, and budget checks pass. Open admission deliberately.
8. Play one bounded end-user Run: real GitHub sign-in, participant entry,
   direct Fly WebSocket, real House response, terminal Replay, and public
   result. Record actual usage and cost. Do not generate repeated paid Runs
   to conceal a failed test.
9. Complete the broader deployed acceptance matrix separately. iOS Safari,
   ChatGPT/WebMCP, sustained direct push, restart/re-entry, and populated
   recovery evidence must not be inferred from a Chrome smoke check.

See [the support and acceptance contract](hosted-preview-support.md) and
[the House approval procedure](hosted-house-approval.md). The deployment
acceptance ticket remains open until its required evidence exists.

The attempt to post this latest progress to Linear was rejected because the
connection requires reauthentication. This file preserves the update; it
must not be described as already posted to the ticket.
