# Hosted preview status — 2026-09-07

This is a dated deployment record, not a release certificate. The source
identities below describe the observed checkpoint. Verify later deployments
separately through `/api/deployment`; do not infer their identity from this file.

## Current result

### September 8, 01:30 UTC — recovery verified; launch defect reproduced

The source-`8705db7`, schema-15 prelaunch checkpoint passed the canonical
isolated restore and Controller/Runtime/platform correspondence checks:

- Checkpoint: `0dd11d12-9321-4fd5-9d73-7c9c8c5c33b2`.
- Manifest: `sha256:132249a03179b7d03463ff669106bb1d7bbd91048a3614bcd4d317adbc434cb8`.
- Deployment: `sha256:228476805fbaa661fcfd5e74312d003a6b1e290d755eed785c03c92c45c0101f`.
- Verification scope: empty prelaunch installation only, 15 matching migrations,
  exact Linux image, closed admission, no Auth restore and no provider calls.

Private archives, the database dump and the verification receipt remain outside
Git. The PostgreSQL 17.11 verifier used a temporary local TCP forward to the
verified direct Supabase endpoint because Docker could not route its IPv6
address. TLS passed through unchanged. The forward was stopped after the drill;
no cloud networking resource or database access policy was changed.

CI `34170497094` passed the application-boundary, database-boundary, and
appliance/same-volume restart tests. The subsequent canonical local candidate
failed at a Gateway launch request with `409 operation_rejected`.
An independent isolated test reproduced that exact failure against the deployed
Linux image with no network, synthetic credentials, one CPU and 512 MB. The
Controller returned `hosted_launch_invalid` before retaining any launch binding
or Room creation. Its compiled reviewed catalog omitted Listing `0.6.0` and the
new Cooperative Planner revision, although the frontend and Gateway selected
them. The catalog correction and regression test are in progress; startup
readiness alone does not prove this launch path works.

Fly remains in maintenance: Gateway liveness is `200`, readiness is `503`, and
Runtime/Controller ports are closed after capture. Public launch and House-fill
gates remain closed. Linear ticket `IMO-184` could not be updated because the
connection returned `oauth_token_invalid_grant`; retain this evidence for the
ticket update after reauthentication. No live LLM acceptance is claimed.

The source correction moves the existing reviewed-artifact loader into a small
testable module used by Controller startup, adds Listing `0.6.0` and Cooperative
Planner revision `2`, and retains every previous artifact. Its regression test
compares that actual loader with the checked-in Fly allowlist and verifies that
each allowed House revision resolves. JavaScript development/runtime/diagnostic
checks passed (21 passed, two image-specific checks skipped in this invocation).
The macOS Rust test compile was interrupted after it ceased producing output
and consuming CPU; it is not passing evidence. Linux regression verification
is running offline with two CPUs and 4 GB. The deployed image still contains
the old catalog until a new image is built and verified.

The offline Linux regression did fail against the previous catalog as expected:
the actual loader returned four Listing digests while Fly allowlisted five,
with only Listing `0.6.0` missing. The corrected-source run is pending. This
establishes a regression reproducer, not yet a passing corrected-image result.
The local acceptance probe also incorrectly required HTTP `202` even though
the Gateway returns `200` when lobby launch has already committed. Its status
check now accepts those two documented responses and retains the subsequent
identity/evidence checks. The new status regression failed before correction;
all 15 focused development/diagnostic tests pass after correction. No server
authorization, deadline, retry, or Room behavior was relaxed.

Linear reauthentication subsequently succeeded. Update comment
`9869027a-4436-444a-b58b-977a59b43491` was posted to `IMO-184`, which remains
In Progress. Commit `484d636` is pushed and its Linux image is building from a
clean Git archive with the same pinned Rust and Node base images.

### 23:34 UTC recovery update

The Fly Gateway and Runtime now return `200` from their readiness endpoints.
The Runtime reports source `8705db79cd726504ec0bd4833fd6b949a8b2296b`.
This is a backend startup result, **not successful live gameplay**. Supabase
still has launches and House fill closed, maintenance enabled, and zero Launch
Requests, Runs, and House Agent Assignments. No real provider call was made.

The repeated starts reported by the owner were not normal idle suspension:

- The 23:04 `/bin/sleep 900` boot and 23:05 termination were an intentional
  backup maintenance session.
- The first 23:15 start has no confirmed trigger. Subsequent exits were an
  initialization crash loop under Fly's `on-failure` retry policy.
- The replacement OpenRouter key reached the Machine environment, but startup
  tried to import it under the old immutable credential identity. An offline,
  network-disabled reproduction confirmed rejection of that replacement.
- Automatic retries were disabled during investigation. The original empty
  Runtime and Controller installation was archived, not deleted, on the same
  volume. Its paired prelaunch checkpoint passed an isolated restore drill.
  A second post-crash capture was retained separately; it is not described as
  another verified paired checkpoint.
- The tested image was installed on the same Machine and volume:
  `sha256:cb190eff719756dc012e85a2910625ee278e848bc37ca8ff36675cf94f6670dd`.
  Fresh initialization imported the replacement key. At 23:33:51 UTC both
  readiness endpoints returned `200`. Public admission remained closed in
  Supabase throughout. Restart policy remains temporarily `no`; automatic
  start and stop remain disabled. No extra Machine was created.

This fresh-initialization recovery was permitted only because all activity
stores were verified empty and the original installation was preserved. It
is not a credential-rotation procedure for a populated installation. Such an
installation must retain its authority and history and use a reviewed,
versioned credential/profile change instead of this empty-preview procedure.

Supabase migration `20260907220000` is now applied: 15 migrations. The earlier
14-migration checkpoint remains historical evidence, not a checkpoint for this
new deployment. A new coherent prelaunch checkpoint is still required before
public admission.

CI run `34169332634` failed at the appliance smoke test. Its failure diagnostic
discarded the actual CLI envelope's `room_operation` field. A regression test
first reproduced that omission; all seven diagnostic tests now pass. Nested
operation details remain excluded from logs. This diagnostic fix does not
prove that Room creation succeeds. A separate network-disabled release-image
smoke harness also failed during Runtime startup; that harness result must
not be confused with the directly observed successful Fly startup.

Remaining acceptance work includes the new paired checkpoint, successful
Room creation and same-Room restart/re-entry, bounded real House Agent play,
and final coordinated deployment evidence. The older observations below are
retained with their original scope and are superseded by this update where
they describe the Machine as stopped or the schema as version 14.

### Earlier checkpoint

The [activity site](https://worldstream-demos.vercel.app) is deployed. Its
public catalog and platform HTTP endpoints pass the checks below. The live
game is **not open for use**. The Fly Machine is stopped and new launches
are closed. No real OpenRouter call has been made during this acceptance
attempt.

Current backend tests run locally and in GitHub Actions. They do not use the
stopped Fly Machine. A successful Vercel or local check is not a live end-to-end
gameplay result. Automatic Fly start is disabled; this is an intentional
maintenance state, not an inactivity-driven stop.

Do not infer that gameplay works from a catalog card marked `LIVE`. That
label describes a reviewed activity, not current authority availability.

## Verified deployment

| Item | Observed value |
| --- | --- |
| Activity source | `8705db79cd726504ec0bd4833fd6b949a8b2296b` |
| Vercel deployment | `dpl_CqTMDxWtFeCdG4eqff9Lujax9BCe` — serving the production domain |
| Manual deployment | `dpl_64GkL4EGaS54N3hn9eqqVyx8MGS6` — source `c4333c4`, Ready |
| Applied Supabase migration head | `20260907203907` — 14 migrations |
| Configured realtime authority | `https://worldstream-preview.fly.dev` |
| Web-selected Listing | `blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782` — successor migration not yet applied |
| Activity Client Release | `sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9` |

At the earlier 22:26 UTC checkpoint, activity source `1570442` was committed locally;
its predecessor `5b068f3` was pushed
to `main`. The maintenance fix was published from a clean Git archive without
waiting for a new backend release. It includes the latest game-design release,
`69b01c6`. The stopped Fly Machine still has source
`2b1d604`; the web deployment is not a matching backend release yet.

At the 23:07 UTC follow-up, `8705db7` is pushed to `main`, and the production
identity endpoint reports the source and deployment in the table above. The
web selects Listing `0.6.0`, but Supabase still has migration head
`20260907203907`. Migration `20260907220000` has not been applied. Launch and
House-fill gates remain closed. This is a mixed-version maintenance state,
not a completed coordinated release.

Public checks against the production domain passed:

- `/`, `/join`, and a formation deep link return `200`.
- `/api/catalog` and `/api/results/agent-heist/recent` return `200`.
- Anonymous `/api/auth/session` returns the expected `401 session_required`.
- `/api/deployment` returns the exact source, schema, and release identities.
- The new `/agent-heist-v3/hosted/` and retained `/agent-heist-v2/hosted/`
  paths return `200`; every file matches its reviewed artifact.
- The retired `/agent-heist/` path and `/api/ws` return `404`.
- The Content Security Policy permits the configured direct Fly WebSocket.

Chrome displays the latest catalog and its room-setup form. The account
owner approved GitHub access. Successful sign-in, account creation, and the
authenticated browser session are verified. No Launch Request or Run has
been created.

The owner also tried to create a people-only waiting room. The live operating
state still has `launches_open=false` and `maintenance_mode=true`. The database
therefore rejects the request before insertion. The earlier UI incorrectly
described `formation_unavailable` as a problem with the participant choices.
Source `1570442` maps only the exact closed-launch database error to the
existing `503 temporarily_unavailable` response. Other authorization and
formation rejections keep their existing behavior. The live browser now shows
“The live room service is temporarily unavailable.” The exact Navigator and
people-only attempt was repeated after deployment; Supabase still has zero
Launch Requests and zero Runs. This corrects the explanation; it does not
reopen the game or bypass the launch gate.

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
- The owner reports replacing the exposed acceptance key. Read-only provider
  metadata confirms the replacement is different, with a USD 2 lifetime limit,
  no limit reset, USD 0 usage, and a December 6 expiry. The exposed old key is
  absent from the freshly inspected workspace key list. Do not use the old key or an
  unrelated unlimited key. Keep the replacement out of chat, Git, screenshots,
  browser bundles, and logs; the local file is ignored and owner-only.
- The replacement key has an assigned WorldStream-only guardrail: USD 2/day
  including BYOK spend, one Granite model, one DeepInfra provider, ZDR for all
  model groups, and no paid/free training or free prompt publication. Its
  separate USD 2 lifetime limit remains unchanged. The eligibility preview
  showed one eligible model and provider. Exact canonical model and `bf16`
  variant constraints remain in each Runner request; display names in the
  guardrail UI do not prove those exact pins. Account-wide defaults were not
  changed. The key-scoped policy correction is recorded in ADR 0022.
- Prompt storage and trace broadcasting are off. The replacement key is staged
  as a Fly secret but is not imported into the Controller credential vault.
  Fly still reports it as `Staged` after the backup below. No live inference has
  been attempted. Metadata and
  settings checks are not proof that a provider response will be accepted.
- Retained House limits also apply: USD 2/day, USD 10/month, and at most ten
  model calls per Assignment. These do not cap Fly, Vercel, or Supabase costs.
- The source repository is private. GitHub Actions allowance and overage
  controls are separate from hosting costs. The current CLI credential cannot
  read the account billing summary; a run's zero-valued timing response is not
  proof of a zero bill. No extra billing permission or paid runner was enabled.
  A later browser check verified an Actions account budget of USD 0 with
  `Stop usage` enabled. Fly's billing page still uses automatic invoicing;
  its displayed USD 0 upcoming invoice is not a future cost guarantee.

## Historical test evidence and subsequent checks

### 23:07 UTC: Fly backup and latest release gate

The Fly Machine was briefly started with only `/bin/sleep 900`, automatic
start disabled, and restart policy `no`. Read-only checks confirmed that
ports `8080`, `9410`, and `9420` were closed and no WorldStream service was
running. No bot or provider call was started.

The existing volume was captured as checkpoint
`207418e7-46c9-41ea-8610-2c6cd046adc8` at `23:04:44 UTC`. Runtime and
Controller archives were downloaded into owner-only local storage. Their
SHA-256 values match the capture receipt. The captured SQLite database passes
`integrity_check`; Rooms, transitions, members, timers, observations, Runner
records, and activation records are empty. The checked Controller activity
directories are also empty. Bootstrap authority and installation records
are present and preserved. This is a volume backup, not yet a paired Supabase
checkpoint or a verified restore.

The same Machine was then stopped. Its full original configuration was
restored and read back: image, volume, size, services, and disabled automatic
start are unchanged. No data was deleted or moved out of the installation.
Two earlier Fly CLI configuration requests failed because the CLI duplicated
the image digest. The successful configuration-only update used the Machines
API with `skip_launch=true`; it did not bypass any WorldStream checks.

[CI run 34168136170](https://github.com/imom39a/worldstream/actions/runs/34168136170)
tested exact source `8705db7`. The Supabase foundation passed. The application
foundation failed when the appliance smoke test initially created a Room,
with `setup_incomplete`; it did not reach its same-volume restart. The
dependent canonical release-profile story was skipped. The smoke wrapper
discarded the JSON output that identifies the incomplete setup stage, so
the underlying cause is not yet established. Do not classify this as the
earlier Membership timeout without new evidence.

The follow-up smoke diagnostic accepts only a bounded operator response from
the original failed command. It reports the command, status, error code, and
one of the three known setup stages. It excludes raw stderr, descriptions,
identifiers, credentials, and unknown response fields. It waits for the child
streams to close before examining the result. Six subprocess regression tests
pass; the combined packaging checks pass 31 tests with three explicit opt-in
skips. No retry, timeout, build profile, or gameplay assertion was changed.
This diagnostic is not itself a fix for the incomplete Room setup.

The local Linux AMD64 image for exact source `8705db7` completed its build.
Its embedded source and reviewed Heist v3 assets match that source. All nine
image-enabled entrypoint tests pass. A separate full-process startup check
passed in about 5.6 seconds with one CPU, 512 MB memory, no network or published
ports, synthetic credentials, and a fresh test volume. Peak memory was about
180 MiB; there were no OOM events, no Rooms, and graceful shutdown passed.
Only the owned test container and its fresh volume were removed afterwards.
The image has not been pushed or deployed. These checks establish startup,
not Room creation, restart recovery, or real-provider gameplay.

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

### Latest restart diagnostic and UI release

CI run `34158069116` tested `ad71a1c`. Both foundation jobs passed. The
protocol story again passed initial streams, external-agent Catch-up, and
the fake House endorsement. After the managed restart, `/api/runs/enter`
returned `503 temporarily_unavailable` before the fresh-sync checks. No
acceptance artifact was produced. The failing participant and upstream
service were not identified by that error.

Focused BFF and Gateway controls passed, but did not reproduce this failure.
A separate native probe failed during managed startup, before handoff
issuance. That setup failure is not evidence of the CI failure's cause.
The temporary, CI-only `[DEBUG-reentry-ad71]` probe records fixed stage,
participant role, and HTTP status categories from the original requests.
It adds no retries, reads no response bodies, and does not change production
limits. Four probe tests and the nine focused acceptance controls passed.
Remove the probe once the failing hop is diagnosed. A Gateway `503` alone
still does not identify its discarded native upstream cause.

The separate game-design release was pushed as `69b01c6`. It adds Heist v3,
Negotiate v3, Inspector v2, and Listing `0.5.0`; the old Heist v2 bytes remain
retained. Its rollout requires the matching additive catalog migration and
new Fly image. Earlier image, schema, and UI checks above do not certify
this later release. Gameplay remains closed pending deployed acceptance.

The diagnostic successor `c4333c4` includes that UX release. The activity
site and developer manual are deployed from this successor. Published
manual HTML, JavaScript, and CSS match the tested build. All eight deployed
Heist v3 files match their reviewed artifact; all six retained v2 files
still match the older artifact. This verifies the published UI bytes, not
native gameplay.

CI run `34161999538` tested this exact successor. Both foundation jobs
passed. The canonical protocol story failed after the managed restart:
both participants' database identity and membership requests returned `200`,
then both original Gateway handoff requests returned `503`. This excludes
the Gateway's own `429` response, but not a native Controller `429` mapped
to `503`. Transport failure, another native rejection, or an invalid native
response still need to be distinguished. No acceptance artifact was produced.

A smaller native control on the exact `c4333c4` image passed: both original
participant bindings received valid `201` handoffs before and after the
managed restart. Its first fixture attempt used an unsupported overlay
filesystem; the corrected fixture uses an owned Linux volume without
relaxing filesystem checks. This control has no Gateway, active House
participants, or Platform launch ledger. It is not a reproduction or a
resolution of the full-stack failure.

The next temporary probe covers the original Gateway-to-Controller handoff and
Controller-to-Runtime Membership read. Active native diagnostics are absent
when debug assertions are disabled. Explicit CI gates enable only bounded,
closed status categories. A private CI file bridges the Controller's discarded
stdio; prerequisite fixtures cannot consume the story's trace budget. The
focused Gateway tests (25), Controller integration tests (19), and Node controls
(12) pass. Library lint, formatting, debug-disabled builds, and independent
review pass. This instrumentation does not fix the failure or add retries.

The additive catalog migration `20260907203907` was applied to the intended
WorldStream Supabase project. The three earlier Listing document hashes
remain unchanged. There are 14 applied migrations. Launches and House fill
remain closed, and maintenance mode remains enabled.

The account owner subsequently approved GitHub access. The browser now
shows `Signed In`, the latest room setup form renders, and both Auth and
Platform contain one account. There are still no Launch Requests or Runs.
The earlier consent URL expired; it is not evidence of a failed fresh
sign-in. The exposed OpenRouter acceptance key still showed zero usage
when inspected. The owner subsequently supplied the limited replacement
described above. No inference request was made while checking its metadata.

### Maintenance fix and remaining timeout

CI run `34164903099` tested exact source `5b068f3`. Both foundation jobs
passed. The canonical story failed again after Room activity and a managed
restart. The original Controller trace records two Membership HTTP read
timeouts; the Gateway reports `503 hosted_browser_unavailable`. No Runtime
`429` was observed in this trace. This identifies the failed boundary, not
the underlying cause. In particular, the successful entries happened before
the Room's later history, so restart has not been isolated as the trigger.
Do not fix this by adding an unproven retry or increasing a limit.

The web-only maintenance correction was built from exact Git archive
`1570442` and deployed separately. Its 91 Platform tests and 16 product tests
pass; the build retains the exact v3 and v2 Client artifacts. The archive's
first product-test invocation lacked its source-revision environment variable
and stopped before assertions; repeating with the exact archived commit passed.
Public endpoint checks identify the deployed commit and schema. The signed-in
browser displays the corrected message after the original people-only action.
This evidence does not establish working room creation.

The next House catalog candidate adds Cooperative Planner `2` and Listing
`0.6.0`, with two distinct strategies on the eligible Granite route. All old
canonical documents remain unchanged. In an isolated PostgreSQL fixture,
all 15 migrations, repeated seeds, exact stored catalog bytes, and all 383
pgTAP assertions passed both before and after development seeding. Production
seeding grants no House approval. The candidate is not applied to Supabase
or deployed to Fly yet.

A separate provider decoder correction accepts the documented omission of
optional attempt history only when the existing first-success and exact
route checks pass. Explicit null, malformed or conflicting history, retries,
missing required metadata, and mismatched routes remain rejected. The public
executor regression was red before the fix; all 16 House model tests and
library lint pass afterward. This is fixture-backed compatibility evidence,
not a successful paid model call.

The successor catalog is committed locally as `587cd17`; the provider decoder
correction is `152fbbf`. At the 22:45 UTC checkpoint neither is pushed or deployed.
The combined local checks pass: 92 Platform tests, 16 product tests, 17 script
tests, both Platform type checks, and generated-artifact correspondence. The
three script skips require separate database or container fixtures; they are
not counted as passes. The cloud still has 14 migrations, zero Launch Requests,
zero Runs, and no House approval for `fly-primary`. The real Auth account remains.

A smaller real-SQLite and HTTP control uses the exact Heist Pack, five Genesis
Memberships, and six Transitions. Two concurrent Membership HTTP reads finish
in about 470 ms locally, within the unchanged 750 ms deadline, including full
response bodies and connection closure. Full-history recovery accounts for most
of the measured backend time. This does not reproduce the CI timeout or prove
its cause. The same control with the production release build profile passes:
warm reads take 59–60 ms, the reopened read takes 82 ms, and concurrent HTTP
reads take 197–198 ms. A repeat also passes. This is an optimized macOS test,
not a deployed Linux game or proof that the full CI failure is resolved.

The acceptance harness now selects a single release build for the CLI,
Controller, Runtime, assignment helper, Gateway, and approved House executable.
Normal development still uses debug builds; an explicit closed profile option
allows a comparison in a separate clean checkout. It records source cleanliness
and profile selection without claiming a deployed-image identity. The focused
harness checks pass: 29 tests, with two explicit image-test skips. Every original
story assertion and prerequisite remains. No second verification pass, authority
fence, production implementation, or timeout has been changed by this correction.
The full canonical story is the next required test.

## Remaining work, in order

1. GitHub consent and the real authenticated browser session are verified.
   Keep the account and its identity records; do not reset the database.
2. Complete the current Room-read timeout diagnosis and exact-source tests.
   Keep the already verified key/guardrail restrictions. The authenticated ZDR
   endpoint preview admits Granite but not Qwen/Alibaba; the additive Granite
   House successor must be included in the coordinated release.
3. Finish exact-source CI and local verification. A fake-provider result is
   useful integration evidence, not a real-provider pass.
4. Deploy a verified matching Fly image with `--ha=false`. A local `c4333c4`
   image passes startup under 512 MB; it does not pass full acceptance yet.
   Any further source correction needs its own exact-source image. Keep launches
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
