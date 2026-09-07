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
- The source repository is private. GitHub Actions allowance and overage
  controls are separate from hosting costs. The current CLI credential cannot
  read the account billing summary; a run's zero-valued timing response is not
  proof of a zero bill. No extra billing permission or paid runner was enabled.

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
it is not exact-source release evidence.

### Follow-up checks and restart correction

The updated manual is deployed at
<https://worldstream-manual.vercel.app>. Its lint, nine tests, 28-page link
check, and production build passed. Published HTML and JavaScript match the
isolated `18bc58e` build.

The stronger standalone Heist browser proof also passed in that isolated
source archive. It verified exact Client Release bytes, CLI Room setup,
protected handoff, authorized Navigator state, live readiness, fragment
removal, and cookie-based reload. It made no platform-auth or provider calls.
The copied native binaries still report source `2b1d604`; this is a behavior
check, not a final-source binary certificate.

Source `342370e51081586c29294a9c8a9cb754a1ed78ab` produced a local Linux AMD64
image with exact embedded source identity. All eight entrypoint regression
tests passed. The full image became ready in about five seconds with network
access disabled, synthetic credentials, a fresh Linux volume, and a 512 MB
memory limit. Measured peak memory was about 170 MiB, with no OOM events.
Room inventory was empty and graceful shutdown passed. This is startup
readiness, not a load, gameplay, or recovery result. The image was not pushed
to Fly.

CI run `34152563941` passed both foundation jobs, including the appliance and
same-volume restart check. Its canonical local candidate then failed in a
Supervisor lifecycle test before it reached the browser story. A status
read could reap the exiting child while an accepted restart was waiting for
its worker, changing `Stopping` to a terminal state and losing the restart.

The correction makes reconciliation leave `Starting` and `Stopping` to the
accepted operation's worker. It does not change APIs or production timeouts.
A test-only scheduling gate reproduces the exact ordering through HTTP
routes: the regression failed 20 out of 20 times before the correction and
passed 100 out of 100 after it. The corrected Supervisor library passed 104
tests; the original test passed 1,600 concurrent stress runs. Production
library lint and formatting passed. Broader test-target lint still reports
unrelated existing findings in other Supervisor test files.

The correction was pushed as
`1beae87b97b2b98716a00e6bc17a8f9ae271a735`. Its Vercel production deployment,
`dpl_5beuQHU49NojJzoLEFfyFnMYqhsg`, is Ready and passed the same public
endpoint and exact-source checks. A matching local Linux image passed all
eight entrypoint tests and isolated full-process readiness in about five
seconds. Peak memory was about 176 MiB under the 512 MB limit, with no OOM
events, no Rooms, and no network or provider calls. This image was not pushed
or deployed to Fly.

CI run `34154853834` passed both foundation jobs and the 104 Supervisor tests.
It reached the actual local protocol story: participant and spectator streams,
an external-agent disconnect and Catch-up, and a fake House endorsement passed.
After the managed Runtime restart, the external-agent stream stayed
disconnected and the test timed out waiting for `commit_move`. No acceptance
artifact was produced. The test had ignored reconnect results and accepted
cached Projections as proof of re-entry; its earlier success log was therefore
not valid evidence of post-restart synchronization.

The follow-up test correction explicitly re-enters both participants into
their original seats. It requires each reconnect to return live and emit fresh
synchronization, rejects cached-state success, and closes every session on
failure. Safe error categories identify the failed reconnect without logging
private frames or credentials. Production transport and timeouts are unchanged.

The two false-positive regressions failed with the old helper. The corrected
focused file passed nine tests in a clean source archive; its full-stack story
was skipped because the local stack was not started. Both Platform TypeScript
checks passed. The actual post-restart story still needs a new full candidate
run; focused helper tests do not establish that its disconnect is resolved.
A matching image must follow the committed correction. None of these checks
is a real LLM, rendered-browser, or deployed gameplay acceptance result.

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
