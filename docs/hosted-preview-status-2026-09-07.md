# Hosted preview status — 2026-09-07

This is a dated deployment record, not a release certificate. The source
identities below describe the observed checkpoint. Verify later deployments
separately through `/api/deployment`; do not infer their identity from this file.

## Current result

### September 8, 08:18 UTC — Room creation restored; live gameplay still unverified

Vercel and Fly are on `6960d4c1928f6d2d63ea6ab5938165b77f65ed91`.
Vercel deployment is `dpl_9mJZKPnKRF41Wrxeb6mqkHhyvjQ9`. Supabase schema
head is `20260908074827`, with Listing 0.9.0, Template 4, Planner 5 and
Auditor 4. The polished Heist v4 client is unchanged.

Fly is started, with its normal entrypoint, one shared CPU, 512 MiB, and the
existing 1 GiB volume. Public `/readyz` returned 200. An eight-digest catalog
allowlist exceeded the old generic 512-character environment guard and caused
startup failures. A temporary three-digest allowlist permits the current
Listing and both earlier live Room Listings. All installed artifacts remain.

The permanent fix is committed as `2e4c51da6d58c0e56c72876a6536bf0343062596`
but is **not deployed yet**. Its exact local image
`sha256:365b5b48ed07f441f8539aa9fd286c02a682f415a866d507db8d35bfd3c1829f`
passed a network-isolated launch with the full eight-digest deployment
allowlist and two synchronized WebSocket participants. Startup was 4683 ms;
peak memory was 215085056 bytes, with no OOM. No LLM call was made. The
managed-agent executable digest still matches the approved Template 4 binary:
`blake3:6c57b11a250c88daf547cd9c5b2d3abcc3ee36e469b2fd81ac7efc29801f41c7`.

The four historical House reservations were released only after offline
shutdown, a populated volume checkpoint, durable per-unit retirement fences,
and exact terminal-evidence checks. No allowance was reset or history deleted.
See [the retirement procedure](hosted-house-retirement.md). Retirement remains
an operator procedure, not automatic capacity recycling.

Operating generation 9 has launches and House fill open. The new browser test
created launch `30992bd9-06f6-472e-b739-34b1887e2567`, Runtime Room
`01M201ARVE75VQ6RP09Y4H2AAA`, Run `dde98b8b-3359-4229-b7ea-1195e91be6e1`,
and public ID `d83f0382c3641af8a6ed26d0ea1d17cd`. This proves Room creation,
not completed gameplay. Browser participation then became blocked by the
locked Mac. Do not create another launch simply to resume this check.
The latest allowance read contains no new provider attempts for this Room.

An expired browser session also produced repeated waiting-room GET 401s.
Reloading the same waiting room recovered access. The confusing fallback
message needs follow-up; no auth fix has been validated for that behavior.

Remaining release checks: finish the signed-in participant and House Action
flow, verify its terminal result, then deploy the tested permanent allowlist
fix with a safe drain/checkpoint. Provider completion alone is not acceptance.
The USD 2 non-resetting provider-key cap is unchanged. No infrastructure was
expanded. The earlier entries below are historical, not current operating state.

### September 8 — corrected transport deployed; bounded live test in progress

Both live services now report source
`cdf801c824404a8dcc0426c381f7b18a323ccc04`:

- Vercel: `dpl_Q3jhrdDXpG3NtjPLLCBdt1xrFEzX`, published from local prebuilt output.
- Fly image manifest:
  `sha256:5bb01e047b38608d9e4a6f1484238746b9998916e9683a1781ba9823996f67a7`.
- Supabase head: `20260908065533`; active Listing `0.8.0`.

The exact local image passed the network-isolated, two-participant launch test
under one CPU and 512 MiB. Startup took 4766 ms; observed peak memory was
271851520 bytes, with no OOM. This test makes no provider call. Its build used
the retained local Rust builder cache `worldstream-hosted-builder:1a91134`
and pinned Node runtime; it is not a fresh dependency rebuild.

Before updating Fly, the populated volume was captured in checkpoint
`dc067ed9-3b48-4b2c-8a92-b27984aa0cfa`. The existing completed Room, Runtime,
Controller, and allowances were retained. This is a volume checkpoint, not a
paired Supabase restore proof. The same one-CPU/512 MiB Machine and 1 GiB volume
remain in use, with normal entrypoint and no sleep override.

Installed Template 3 and Planner 4 / Auditor 3 were captured and approved against
the actual executable digest
`blake3:65c0fe58d3557d56401ddc8c363f358d19246daaa80f8241e8482486b201065f`.
Approvals were inserted unavailable, then separately activated after public
readiness succeeded. Launches and House fill were reopened at operating
generation 4. The provider key reported USD 0.0000095 usage and USD 1.9999905
remaining under its USD 2 total limit with no reset before this test.

The real browser test is launch `d7e1d637-5e78-4985-b6a3-a3debbce2c54`,
Room `01M1ZXRDH145SNF0M7FRCKP2FE`. Navigator entered, inspected `route`, and
published `route_roof`. It completed with `failure / no_strict_majority / 0`
and public result `33196167e981e4b99188854dd5ee7650`. Both new House processes
were later observed exited with status 1. The initial allowance reads had not
yet shown their calls; those early reads did not prove they exited before
dispatch. Thus **live LLM acceptance still fails**. At operating
generation 5, human launches remain open but new House fill is disabled.

The existing allowance ledger retains three ambiguous attempts for each old
Room House unit; none is a verified completion. Inspection of the new units
found retained leased Activations. A bounded helper-only diagnostic later
observed lease-expired followed by no-Activation for each unit, without calling
a provider. This is not proof of the original exit cause; local protocol
diagnosis is in progress. Do not treat process readiness as gameplay proof.

A further diagnostic launch `7a3ecc66-3fd7-4028-8bea-893263d231b7` failed
before Genesis with `runner_capacity_unavailable`. The four reservations from
the two completed matches remain conservatively retained. No capacity limit
was raised and no reservation, allowance, or historical result was erased.
House fill was briefly enabled for that check and is disabled again at
operating generation **7**; human launches remain open. A safe release needs
verified shutdown evidence, not a blind database capacity reset.

The local instrumented image diagnostic is not acceptance evidence: it used
an isolated helper wrapper to record only protocol result codes, synthetic
credentials, and `--network=none`. It could launch a Room and accept a human
clue inspection, but did not reproduce the live House exit or prove a model
turn. Its repeated `assignment_activation_none_available` responses must not
be counted as successful agent participation. Owned diagnostic containers and
their fresh volumes were removed; private diagnostic scripts remain outside
the repository for continued investigation.

At the final key-metadata check in this checkpoint, OpenRouter reported total
usage USD **0.00091845**, remaining USD **1.99908155**, and no reset on the
USD 2 key limit. This cumulative amount must not be attributed solely to the
tiny diagnostic: ambiguous requests can still be billed. It is provider usage,
not a measurement of total Fly/Vercel/Supabase charges.

A later ledger read established two **validated provider completions** for the
second Room: Planner used 1505 input / 789 output tokens, cost USD 0.000284670;
Auditor used 1517 / 768, cost USD 0.000283020. Planner also has one
`provider_failed` attempt. The ledger modification time was 07:14:55.804Z.
Neither validated response became a recorded Runtime Action: the Room's only
Action receipts are the human inspection and publication, while both House
Action-operation ledgers remain `prepared` without remote acceptance.
The immediate investigation is therefore the Action handoff after model
completion, not an assertion that OpenRouter was never called. A local
protocol regression is being built for live-frame interleaving during Action
synchronization; this hypothesis is not yet a proven production root cause.

That regression reproduced `InvalidDaemonData` before Action submission in
0.08 seconds. The correction accepts only typed deliveries for the exact
Room and Membership while awaiting synchronization or an Action receipt.
It consumes the existing byte/message/time budgets, does not acknowledge
frames or advance a Cursor, and does not change the Action's original Head
precondition. A cross-Membership delivery remains rejected. All 13 Action
gateway/orchestration tests pass. This correction is not yet deployed, and
production LLM-to-Action acceptance remains unverified.

### September 8 — first live result; OpenRouter TLS half-close isolated

Vercel `eebb0a577b91d606e6c036f20ca4d3872e26a65a` is deployed as
`dpl_Gz7K9eZY55WmvVaeEmbSGPrp1T4T`; Fly remains on `c07a7d5` below.
Reopening the same waiting room resumed the missing House startup. Both
House processes became ready, and the human browser advanced from Lobby to
Briefing and Negotiation. Through the actual UI, Navigator inspected `route`
and published `route_service`. The Room reached Complete at sequence 9 with
`failure / no_strict_majority / 0`. Its Replay-verified summary appeared in
`/api/results/agent-heist/recent`, public ID
`bb93289cdaeb3f47865c9597254e7231`. This proves human action, live state,
timers and result publication, **not successful LLM participation**.

Each House ledger recorded one ambiguous provider attempt and no validated
response. A zero-cost TLS diagnostic against OpenRouter's public model index
isolated the failure: the production-style raw TCP write-half-close returned
zero bytes and unexpected EOF; keeping both directions open returned HTTP
200 and 705648 bytes. The correction removes that premature shutdown while
retaining certificate checks, fixed origin, request/response limits and caps.
An explicit network regression uses only an invalid fixture key and expects
an authentication rejection, never a paid completion.

The pending executable change has append-only successor metadata: Runner
Template 3, Planner 4, Auditor 3 and Listing 0.8. Existing Listing 0.7 and its
House definitions remain available for the completed Room and its results.
The Pack and v4 client are unchanged. Old executable approvals must not be
silently rebound to new bytes; prepare exact new approvals after installation.
No new listing is enabled until the corrected image and approvals are verified.

A 32-output-token route diagnostic from Fly returned HTTP 200 from DeepInfra
and reported USD 0.0000095 cost. It was a provider connectivity check, not an
in-Room Action. Its requested and selected endpoint model were the dated
Granite slug; the top-level response used `ibm-granite/granite-4.2-8b`.
The [OpenRouter endpoint catalog](https://openrouter.ai/api/v1/models/ibm-granite/granite-4.2-8b-20260831/endpoints)
confirms that exact canonical ID. The decoder now permits only that explicit
mapping, while still requiring exact requested/selected model evidence.
The catalog had two endpoints but only DeepInfra was available and selected.
[Router metadata](https://openrouter.ai/docs/guides/features/router-metadata)
counts candidates separately from attempts; total candidates need not be one.
The decoder still requires one available selected endpoint, attempt 1, the
pinned provider, and no fallback or transformation. Changed model/provider
evidence remains rejected by the regression matrix.

Supabase migration `20260908065533` applied locally and remotely and retained
the existing Run. It adds two House definitions and one Listing, with no
operating approval. The security advisor reported no error; the existing
leaked-password-protection warning remains (GitHub OAuth is the tested path).

### September 8 — live WebSocket restored; House startup recovery corrected

Fly now runs source `c07a7d58338d16db6336ee74920907451a48e506`, image
`sha256:5ff225831681e58efbe2114e20a6277f84659b152fb9dc425e42ad2438d4ea6d`.
The normal entrypoint is active on the original one-CPU/512 MiB Machine and
1 GiB volume. Public `/readyz` returns 200. A real public WebSocket upgrade
returns 101; signed-in Chrome displays the authorized Navigator Projection,
Live connection, three crew members and Lobby state. The four ignored Fly
transport metadata headers are not trusted as identity or forwarded upstream.
All 24 gateway boundary tests pass. The exact release image also passed the
isolated two-participant launch probe with no OOM or provider access.

The populated Room was captured before this deployment in Host checkpoint
`5f43c1eb-ef56-4f0e-813a-6bfee6a05c03`. This is a volume capture, not a
paired Supabase restore proof. No Room or allowance was reset.

The first live Room exposed a separate coordination bug: `room_setup_complete`
was treated as the end of startup, even while the Host reported
`waiting_for_readiness`. Only one House runtime binding had been created.
Recovery now resubmits the exact frozen launch in that waiting state, while
still allowing human entry (needed for synchronization). It does not re-freeze
the roster, authorize new work, or retry completed/needs-attention launches.
The regression fails before the correction; all 95 platform tests pass with
one explicit skip, and platform type checks pass. Deployment and full gameplay
verification of this additional platform correction remain pending.

### September 8 — public formation works; Fly WebSocket headers block entry

Source `1a911340b57f3fdcd7a886776a70f2e683c714d2` is deployed to Vercel as
`dpl_BqZQRr2MvKiJDnEvWnsSWrn1Ld5v` and Fly as image manifest
`sha256:5ff4b0498aaf0530e63ef4efe78894144f0a88bafb5270a79786fba3feece5a7`.
The image passed the isolated two-participant WebSocket launch test under
one CPU/512 MiB, with no OOM and no provider access. Both public deployment
identity endpoints agree, and Fly `/readyz` returned HTTP 200.

The retained Runtime could not start because its immutable base Distribution
contained seven embedded revisions while the new build adds Heist 0.3. The
daemon correctly rejected that identity mismatch. The captured SQLite
database had zero Rooms, Supabase had zero launch requests, and the Host
creation/assignment ledgers were empty. After checking the live database hash
against the audited capture, the empty Runtime directory was preserved at
`/var/lib/worldstream/archives/empty-runtime-before-heist-03-1a91134` and a
fresh Runtime directory was initialized. Nothing was deleted. The Controller,
credential vault, Supabase database, accounts, and approvals were preserved.
This is a zero-history initialization recovery, **not** a supported upgrade
for an installation with Room history. That upgrade path remains unresolved.

The pre-initialization capture `5cae8f36-1a5f-49b0-a3ba-4fb51e4ae4b0` was
downloaded and its archive hashes verified. It is not a paired restore proof
for the new Runtime, and it predates the live Room described below.

Exact House approvals were prepared from the actual installed profiles,
Template 2 and executable, inserted unavailable, then activated separately.
The provider key metadata still showed USD 2 remaining, no limit reset, and
zero usage before the browser test. No credits or extra resources were bought.

Real signed-in Chrome created launch
`daca3af7-f135-4d95-9156-e413423190e9`. After the 30-second fill window,
the platform created one Run with a human Navigator, Cooperative Planner
Insider and Skeptical Auditor Broker. The standalone v4 client opened but
reported `Realtime connection failed`. A public upgrade probe returned 403.
The Gateway rejects all `X-Forwarded-*` fields, including the transport
headers added by Fly Proxy. See
[Fly request headers](https://fly.io/docs/networking/request-headers/).
The correction must ignore only the four standard Fly transport metadata
fields, keep actual Host/Origin and ticket checks, and never relay or trust
those metadata values as identity.

New launches and House fill were closed again while this fix is tested;
maintenance is off, the Runtime is healthy, and the existing Room is retained.
There is no successful live LLM/gameplay/terminal-result claim yet. In
particular, **do not repeat the empty-Runtime recovery now that a Room exists**.

### September 8 — retained-volume import conflict identified and tested

Runner Template 2 reused Template 1's `hosted-house-01` instance identity.
The installed registry requires unique instance IDs across every retained
revision, so the complete startup import was rejected. Both templates name
the same executable digest; changed executable bytes were not the cause.

The successor now uses `hosted-house-r2-01` and a distinct loopback health
listener, `127.0.0.1:9592`. Template 1 and its retained data stay unchanged.
A new regression test failed before this change and passed after it. The
complete import preview, including profiles and client, then passed against
the actual Fly volume with the proposed successor. This preview did not
install or approve execution. The focused Node suites passed 15 tests with
three explicitly skipped environment-dependent tests.

The temporary diagnostic shell/sleep startup override was restored to the
normal image entrypoint and bounded restart policy. The Machine remains
stopped while the corrected image is prepared. Launches and paid calls remain
closed. A published image and real end-user match are still required.

### September 8, 05:48 UTC — builds published; retained-volume startup needs repair

The exact source `81a2cbaef690cd226c9c5b62e78600bc7d7831d0` release image
built locally and passed the two-participant direct-WebSocket hosted launch
probe at one CPU/512 MiB with no network/provider access. Startup took 4.5 s,
both clients synchronized, one setup ledger entry was created, and peak memory
was 275,496,960 bytes with zero OOM events. This is not full gameplay or a
House-provider acceptance result. Its local image ID is
`sha256:a05a7507c2fb28371e7f6deb88e0e34aa1638b317ccf8cf3d6f75b70e09edae3`.

Fly received the same image, registry manifest
`sha256:3bdbe9344dd6c70f2e26d6314b6af4e3ddaefbd97606eb7e3d369f856fdb0e6c`,
on the existing Machine and 1 GiB volume. Its guest remains one shared CPU and
512 MiB; autostart and autostop are disabled. The CLI set a bounded on-failure
policy with three retries. Startup then failed at
`hosted_ctl_init_--runner-template_failed` against the retained installation,
and the Machine stopped after its retries. An explicit start reproduced that
same failure. Do not restart repeatedly or claim the Runtime is operational.
The next task is to inspect the rejected initialization import against retained
state. Fresh-volume launch success did not verify this upgrade path.

Vercel production is now `dpl_6uBH3a9DBvs8GPpxcPimAqAffCGa`, published from
local prebuilt output. `/api/deployment` verifies the exact source, schema
`20260908040223`, Listing 0.7, Pack 0.3, and Client v4 identities. The first
CLI deployment lacked Git-specific runtime identity. Republished the same
artifact with documented GitHub CLI metadata; no provider environment value
was fabricated or set as a project secret. See Vercel's
[CLI metadata guidance](https://vercel.com/kb/guide/branch-variables-and-domains-not-linked-to-cli-deployments).

The maintenance marker remains present. Platform launches and House calls stay
closed. No paid provider call, purchase, upgrade, extra Machine, volume, or
database resource was created. The public UI is deployed, but live matches are
not yet operational.

### September 8, 05:41 UTC — local web build ready; provisioning means Room busy

The corrected native smoke still returned `setup_incomplete/member_capability`
on its fourth attempt after three passes. A further opt-in probe captured the
closed response error code: HTTP 429 **`room_busy`**, not `rate_limited`.
The limiter locking defect is real and regression-tested, but it did not fix
this setup symptom. Do not describe it as the provisioning root cause.
Temporary native probes have been removed from the working source again.

The hosted operation backend explicitly resumes an existing Room Setup Operation
rather than creating another one. The remaining acceptance check is that this
normal bounded lifecycle contention recovers through the actual hosted launch
flow. The failed one-shot CLI smoke is retained as failure evidence; no retry
was added to it and it is not being relabelled as passed.

Remote Supabase migration `20260908040223` is applied and verified. The new
Listing exists exactly once. Launches and House fill remain closed, maintenance
is active, and there are no retained Runs. This was the only pending migration;
it adds immutable metadata without changing Auth, RLS, limits, or approvals.

The local Vercel production build from clean source `81a2cba` passed in an
isolated checkout, including exact Heist v4 artifact verification and retained
v3/v2 builds. The downloaded local environment had an empty Git revision; only
that empty value was removed so the build reads the real checked-out commit.
Secret placeholders were not replaced or uploaded as project settings. The
Fly release image is still building locally. Neither new build is deployed yet.

### September 8 — MVP deployment proceeds from local, not CI

The user confirmed that the priority is a working live application and that
CI/CD is not required for this MVP. Build, test, and publish from the local
operator checkout. CI run `34190315614` was explicitly cancelled and is now
terminal with conclusion `cancelled`; its earlier Supabase prerequisite passed.
Do not treat cancellation as a passing acceptance result or start replacement
CI work as a deployment gate.

The local native probe reproduced member provisioning returning HTTP 429.
The Runtime limiter samples time before locking its shared state, allowing
concurrent admissions to apply samples out of order. A regression test fails
on that ordering and passes after moving sampling inside the lock. All 16
limiter tests pass, including real clock regression and quota enforcement.
The causal check against the original appliance flow is still in progress.
Production has not yet received this change. Cloud resources and spending
limits are unchanged.

### September 8, 05:26 UTC — intermittent member setup reproduced locally

The next ten-run local debug smoke loop stopped on attempt four after three
passes. The original Room creation failed at `member_capability`, with retained
attention `daemon_result_ambiguous` and `retryable: true`. This reproduces the
CI symptom on the corrected mounted-volume harness. It does not establish
whether the original request committed, or justify retrying it as acceptance.

An opt-in, debug-build-only native diagnostic is being prepared to distinguish
transport failure, response status, and response decoding without recording
request bodies, credentials, or identifiers. There is no behavior fix yet.

Commit `77e216a862628659529cd31890fffa8d53352384` is pushed to draft PR #2.
Diagnostic CI run `34190315614` is active; its Supabase identity prerequisite
passed. GitHub's Actions budget was observed as $0 with Stop usage enabled;
no spending controls were changed. Production remains closed and no provider
calls, deployments, remote migrations, or resource expansion occurred.

### September 8, 05:17 UTC — corrected debug smoke repeats pass

All five additional debug smoke runs passed Room creation, same-volume restart
and credential export, following the first successful debug run and release-image
run. The original intermittent CI setup failure was not captured. The three
active diagnostic containers were stopped with their data preserved.

The temporary smoke diagnostic also supports the explicit local option
`WORLDSTREAM_SMOKE_RETAIN_FAILURE=1`, which preserves its owner-only fixture on
failure for inspection. Default cleanup behavior is unchanged. Eight focused
diagnostic tests still pass. No retries, timeouts, production code or release
criteria changed; the next CI attempt must retain enough evidence to distinguish
the original failure, not count these local repeats as full acceptance.

### September 8, 05:14 UTC — local harness mismatch identified and corrected

Direct startup against the preserved private fixture identified the harness
error: SQLite rejected Docker's `overlay` filesystem and required ext4 or xfs.
This was not a credential failure. The earlier debug-symbol and user changes
did not establish a cause and must not be treated as fixes.

The release-image smoke now places temporary fixture data on the image's
mounted volume using `TMPDIR=/var/lib/worldstream`. The original smoke passes
Room creation, same-volume restart, and explicit credential export. The separate
debug build also passes when `/tmp` is an explicit Docker volume. No filesystem
validation, protocol boundary, retry rule or timeout was relaxed. A five-run
debug smoke repeat is active to try to capture the intermittent setup failure.
CI's original failure remains unresolved; these local passes are not full
gameplay or production acceptance. No cloud changes or provider calls occurred.

### September 8, 05:08 UTC — bounded setup diagnostics prepared, cause unresolved

The smoke script now has temporary `[DEBUG-member-setup]` instrumentation that
extracts only a reviewed retained attention code and its expected retryability.
It never emits the record's descriptive text, identifiers, seats or authority.
Eight focused tests pass, including malformed/private-value rejection. This
instrumentation changes neither setup retries nor deadlines and must be removed
when the diagnosis is complete.

A separate offline Linux build completed, but the local script-based smoke
failed earlier at `hosted_ctl_server_start_failed`. Running unprivileged and
using the already verified release executables did not reach the relevant
setup stage. A leftover diagnostic Controller was cleared by stopping only the
owned container. These are harness failures, not reproductions or explanations
of CI's `member_capability` failure. Diagnostic containers and build caches are
retained locally; none is a production image or new acceptance certificate.
No new CI run, cloud deployment or paid call was started.

### September 8, 04:53 UTC — full candidate CI fails at appliance setup

CI run `34186167910` completed with failure at 04:50:50 UTC. Its two prerequisite
jobs passed, but the canonical candidate's repeated appliance smoke stopped at
`room create`, `setup_incomplete`, `member_capability`. The full gameplay story
did not run and no local acceptance evidence artifact was produced. The job's
live browser log exposed this failure before its downloadable log was available;
earlier running-status observations must not be read as gameplay progress.

This matches the initial partial setup from the local release-image smoke.
Ten later setups on that running image passed; a separate fresh, network-disabled
instance also passed its first setup. A bounded repeated local probe is now
trying to capture the exact retained attention reason before any resume.
All 100 subsequent attempts on that fresh local instance completed. Together
with ten earlier warm attempts and its first successful setup, this does not
reproduce the intermittent failure or prove it fixed. Both diagnostic containers
were stopped with their volumes retained. The next diagnostic must distinguish
transport ambiguity, unavailable credentials and explicit Runtime rejection
without exposing authority values or replacing the retained failed operation.
No cause, fix, full acceptance or deployment is claimed. Production remains
closed and unchanged; no paid provider call was made.

### September 8, 04:40 UTC — connected-Lobby reproduction passes on release image

The earlier connected-Lobby diagnostic was updated only for the candidate's
source, Listing and Client identities and run against its exact release image.
Two synthetic participants used real Gateway handoffs, session redemption,
one-use stream tickets and direct WebSockets in a network-disabled container.
Both synchronized, each received one live Observation, and neither recorded a
protocol error. The Controller reported `launched` after one launch attempt;
the Gateway returned 200 with `lobby_launch_committed` true.

The operation retained exactly one hosted-launch record, one Room creation and
one task setup. Gateway and Runtime readiness remained 200 after a monitor
interval. Peak cgroup memory was 220,921,856 bytes under a 512 MB hard memory
and swap limit, with zero OOM events. The isolated container exited cleanly;
only its newly owned synthetic test container and volume were removed. The
protected diagnostic receipt was retained outside Git.

This verifies the original connected-launch path on the new image. It is not a
House Agent, full game, restart/re-entry, real Auth or production acceptance
result. The separate full canonical CI job remains active. The earlier CLI
partial setup was not reproduced by this Gateway path; its cause is still
unestablished, and its successful same-operation resume remains the only claim.

### September 8, 04:38 UTC — exact release image and bounded startup smoke

The committed candidate's Linux amd64 release image built successfully in
9 minutes 40 seconds. Its local image ID is
`sha256:1ab0058e632623c6ffaa53a4426ec780a5c18dd98f13fc8cf8059c781f2b2248`.
This is a local image identity, not a registry manifest or deployed Fly image.

The unchanged image started with one CPU, 512 MB memory, networking disabled,
an isolated local volume and synthetic unusable provider credentials. Health,
readiness and version endpoints returned 200; version identified source
`856a28dce72efb2159db8f033eb438e6b8dfc346`.

The CLI example resolved Heist 0.3.0. Its first Room creation stopped at
`member_capability`; an explicit resume of that same retained setup operation
completed it. The reason for the initial partial setup is not established.
After a container restart, readiness returned to 200 and inspection verified
the same sequence-zero Genesis hash and authoritative state hash, with healthy
integrity. No out-of-memory kill was reported.

This smoke is basic startup and Genesis retention evidence, not active gameplay,
participant re-entry, public formation or a real-provider result. The full
canonical CI acceptance job remains active. Production has not been changed.

### September 8, 04:28 UTC — live maintenance display and release preparation

The public catalog still advertises Heist as `available` with “Ready for live
formation.” A direct public Fly readiness request timed out after 15 seconds.
Fly's Machine API shows the existing Machine **started**, with its readiness
check reporting 503, consistent with the retained maintenance closure. This
is not evidence of another Machine restart. The catalog route currently bases
availability on configured dependencies, not their operational readiness;
its message must not be used as proof that live formation is open.

A local release image build is running from a Git archive of committed candidate
`856a28dce72efb2159db8f033eb438e6b8dfc346`, using the same pinned Rust and Node
base images as the prior deployment. The archive excludes untracked secrets and
uncommitted status notes. CI's canonical acceptance job is also still running.
Linear IMO-184 received the preceding verification update successfully.

### September 8, 04:24 UTC — candidate CI prerequisites pass

Candidate `856a28dce72efb2159db8f033eb438e6b8dfc346` is on draft
[PR #2](https://github.com/imom39a/worldstream/pull/2), not production main.
[CI run 34186167910](https://github.com/imom39a/worldstream/actions/runs/34186167910)
has passed the Supabase private identity boundary and Hosted application
boundaries jobs. The latter's logs explicitly confirm the Controller catalog,
exact installed Pack rules, and clock-safe CLI Room example tests passed.

The separate offline Linux server catalog test also finished successfully:
one test passed, with 181 filtered out. This diagnostic container is not a
release image. The canonical browser-to-result CI job is still running;
Lobby launch, restart/re-entry and final result acceptance are not yet claimed.
No production promotion, remote migration, House approval or paid model call
was performed for this candidate.

### September 8, 04:11 UTC — local database and catalog verification

The local `supabase_db_agent-streamer` database now has migrations through
`20260908040223`. The normal migration command applied the earlier pending
Granite metadata migration and the new clock-safe metadata migration. A query
verified one new Listing, two Template-2 House definitions, and zero approvals
for those new definitions. Remote Supabase and cloud services remain untouched.

`supabase test db --local` passes all 388 tests across eight files, including five
new assertions for the clock-safe Pack/client/projector, House budget and
immutable Listing. The Controller catalog test also passed through its fully
qualified compiled test name (one test, not the earlier empty exact filter).

Additional source tests now check that every reviewed Listing resolves to exact
installed Pack rules and that the new CLI Room example resolves through its
actual revision catalog. These new tests still need a refreshed Linux test build;
the current diagnostic container is compiling the server catalog test from its
earlier input snapshot. Do not treat that snapshot as full candidate acceptance.

### September 8, 04:06 UTC — successor release wiring and local checks

The candidate now selects Listing `0.7.0`, Heist Pack `0.3.0`, browser Client
Release v4, result projector `0.3.0`, Cooperative Planner revision 3 and Skeptical
Auditor revision 2. Both House definitions refer to Runner Template revision 2;
their provider route, privacy requirements and spending allowances are unchanged.

Exact candidate identities:

| Artifact | Digest |
| --- | --- |
| Pack 0.3.0 | `blake3:4e4c970403f29a8448a1a3bcf7a96c030df713499730288f324c7e200d160b2d` |
| Listing 0.7.0 | `blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630` |
| Client v4 | `sha256:12714052c8e1cac59a95b0439e8e86bf17766c8f689f5782d1dd5d4c0efd4cd6` |
| Projector 0.3.0 | `blake3:f676cab8007a374db5510d66f534697472c53ea4b2719900bfecce53786f7563` |

The original v3 browser build was verified against its existing artifact digest
before retention. Product builds now install separately checked v4, v3 and v2
artifacts at their own URLs. Retained Listings keep their original client and
exact allowed House definitions. Result reconciliation now explicitly resolves
all six reviewed Listings through the matching old or new projector.

Completed local checks:

- Heist client: 35 tests and production build passed.
- Platform: 94 tests passed, one existing live test skipped; TypeScript passed.
- Activity Client identity checks: nine Releases, five Deployments, nine Bindings.
- Browser surface checks passed for new Heist, retained v3, Negotiate and Inspector.
  These check unauthenticated/empty entry behavior, not a connected match.
- Hosted-development script tests: 23 passed. Packaging tests: 32 passed and
  three environment-dependent tests skipped before adding the migration assertion;
  the House-approval suite then passed seven tests with one database test skipped.
- Product build and developer-manual link checks passed.
- Migration `20260908040223_hosted_clock_safe_heist_revision.sql` was executed
  twice inside one rollback-only local database transaction. Exactly one Listing
  and two new House records existed; operational state and approval rows were
  unchanged. The transaction was rolled back; remote Supabase was not modified.

Linux Host/Controller checks are compiling in an isolated diagnostic container.
The Controller test must be rerun with its fully qualified name or without
`--exact`; an unqualified exact filter is not test evidence. These checks and
the original connected story, restart/re-entry, clean full acceptance, deployment,
checkpoint and bounded real-provider test remain pending. No live admission,
House approval, provider spending, cloud expansion or release readiness is claimed.

### September 8, 03:54 UTC — clock-safe Heist core revision passes locally

The new `AgentHeistLobbyV3` implementation declares Pack version `0.3.0` and
uses a revision-local phase engine with minimal fractional timestamp formatting.
The old `agent_heist.rs` and `agent_heist_lobby.rs` files remain byte-unchanged.
The registry retains the old exact executors and adds the new exact executor;
the new golden transcript is fixed, not generated during normal registry startup.

Linux, offline verification completed successfully:

```text
cargo test --offline -p worldstream-core --features conformance-tracer
188 unit tests passed
6 Lobby integration tests passed
3 public API tests passed
4 registry-bound trace tests passed
0 doc tests; command exit 0
```

The captured `.914230084Z` launch now enters Briefing for both two-seat and
three-seat Rooms, schedules a canonical `.91423Z` deadline, and replays through
the exact new revision. Additional tests cover fractional widths, reminders,
successor timestamps, year rollover, and unchanged old-revision behavior.
The new phase engine also runs the existing Heist privacy/phase tests.

The final source-only test-module lint annotation required re-authoring the new
revision's source-bound golden transcript. The full core command was rerun after
that update and again exited zero with the same test counts. Old source files
were not changed. Clippy's duplicate-module exception is local to the new test
module: it intentionally executes the shared behavioral assertions against a
second executor, rather than aliasing the old executor's tests.
The final source also passes `cargo clippy --offline -p worldstream-core
--all-targets --features conformance-tracer -- -D warnings`, targeted
`rustfmt --check`, and `git diff --check`.

This is **local core evidence only**, from an isolated test builder, not a clean
release build or hosted acceptance. Client compatibility, immutable Listing and
projector revisions, House compatibility/approval, coordinated deployment, the
original connected reproduction, and the separate restart/session `503` remain
to be verified. No production admission or House activation was opened, no paid
provider call was made, and no cloud resources were expanded during this fix.

### September 8, 03:41 UTC — deterministic Heist timestamp regression is red

The captured failed launch retained canonical input time
`2026-09-08T03:37:27.914230084Z`. Repeating its exact input against the isolated
Runtime returned `400 internal`; read-only inspection found Room sequence zero,
Lobby phase, and no committed external-input semantic receipt. Its stopped
synthetic state is preserved privately at
`/private/tmp/worldstream-connected-lobby-failure-LO0kvI`.

The Lobby formatter truncates fractional seconds to microseconds and always
prints six digits when nonzero. This input therefore produces a deadline ending
in `.914230Z`. The Core timestamp contract correctly rejects that non-minimal
fraction. The ordinary Heist phase formatter uses the same formatting pattern.
Retry retains the original input time, so the failed launch cannot recover by
waiting for a different wall-clock value.

A new integration regression in `crates/worldstream-core/tests/agent_heist_lobby.rs`
uses that fixed timestamp with the real registry-bound trace. It failed on Linux:

```text
cargo test --offline -p worldstream-core --features conformance-tracer \
  --test agent_heist_lobby \
  hosted_launch_accepts_a_clock_sample_with_zero_in_microsecond_position \
  -- --exact --nocapture

Activity Pack Reduce operation faulted: invalid Activity Pack output:
timestamp must be a valid UTC RFC 3339 value with seconds and minimal
0-9 digit fractional precision
```

The test itself completed in 0.25 seconds after compilation. This identifies a
deterministic Pack defect behind the isolated launch failure; it does not by
itself diagnose the separate post-restart session `503`. The regression is
intentionally red while implementation is pending. ADR 0010 requires a new
semantic Pack digest for the corrected timer behavior and exact old-executor
retention. Do not weaken Core timestamp validation or silently alter the pinned
Heist revisions. The corrected revision, hosted artifact identities, regression
and full connected acceptance are still required before reopening admission.

### September 8, 03:35 UTC — isolated connected launch failure reproduced

A local, network-isolated probe now exercises the deployed Linux image
(`484d636`, image ID `7a224a56bdb4c28c641900fcdd423d38582e9bfd4c18afc812fafa45cc115fed`)
with a fresh owned volume, synthetic credentials, one CPU and 512 MB. It creates
the reviewed two-seat Heist setup, issues and redeems exact hosted handoffs,
connects the human Navigator and external-Agent Insider through the Gateway,
and acknowledges their Projection Resets. No House Runner or provider is used.

Two valid runs reproduced the Lobby symptom with both participants synchronized
and every required seat ready. The second captured `launch_rejected` on each
readiness sample while launch attempts advanced from 2 to 41. Other runs passed
without a product-code change. This rules out absent direct participation or a
stopped reconciliation loop in this reproduction; it does not establish the
Runtime rejection's cause or prove equivalence to the three-seat CI failure.
The earlier post-restart session `503` remains independently unresolved.

The next bounded probe reads the Runtime response to the same retained launch
input and, on failure, its read-only semantic receipt classification. It never
substitutes a new input, broadens a credential, or changes a production gate.
Private diagnostic scripts and per-run receipts are under
`/private/tmp/worldstream-connected-lobby-*`; failed attempts are retained as
failures. Each completed probe removed only its own disposable container and
volume. These checks are diagnostic evidence, not full gameplay acceptance.

### September 8, 03:18 UTC — connected acceptance stalls in the Lobby

CI `34180489064` for source `194825a` finished unsuccessfully. Both prerequisite
jobs passed. The connected story established all participant and spectator
streams, but Heist remained in `lobby` and the Navigator received no
`inspect_clue` offer within the test's 60-second bound. The new probes observed
successful initial handoff issuance/redemption, session status (`200`), and
Stream Admission Ticket issuance (`201`) for both authenticated participants.

This run did not reach ordinary disconnect, House endorsement, or Runtime
restart. It therefore neither reproduces nor clears the earlier post-restart
`503`. No passing acceptance artifact or live-provider proof exists. The next
diagnostic must inspect the retained lobby launch/readiness assessment with
connected participants, rather than repeatedly rebuilding or extending the
action-offer timeout. Production admission and House activation remain closed.

### September 8, 02:22 UTC — acceptance blocked on post-restart session health

CI `34177427443` finished unsuccessfully. Its application and database boundary
jobs passed. The connected local story passed initial participant/spectator
stream admission, external-Agent disconnect and Catch-up, and a fake-provider
House endorsement. After the retained Runtime restarted, both participants
received new handoffs successfully. The external Agent's subsequent session
health request returned `503 participant_session_unavailable`, so the full
story did not pass and produced no acceptance artifact.

This is distinct from the corrected catalog launch rejection. Investigate the
post-restart session-health path against an Agent with a retained acknowledged
Cursor; do not count successful handoff redemption as synchronized re-entry.
The temporary native diagnostic is debug-build-only, while canonical acceptance
builds release binaries, so its empty trace does not establish absence of a
Runtime error. Public admission and House activation remain closed. No live
provider completion or end-to-end production acceptance is claimed.

Call-path inspection ruled out the legacy Console `health()` attach as the
cause of this hosted failure: hosted status uses `HostedBrowserSessionBrokerV1`
and the read-only Membership-status endpoint instead. The exploratory legacy
health regression was removed, and its unfinished isolated compile was stopped;
neither is passing or failing regression evidence. No legacy health behavior
was changed. The CI-only fetch probe now also covers hosted session admission,
status, and Stream Admission Ticket requests without reading bodies or headers.
Its added coverage test failed before the probe change and all eight diagnostic
tests passed afterward. A new canonical run is required to locate the actual
upstream failure; the original reconnect defect is not yet fixed.

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
with only Listing `0.6.0` missing. The corrected-source Linux run subsequently
passed: one test passed, none failed, in 0.05 seconds after compilation. It
exercised the shared Controller loader with corrected source overlaid on the
cached Linux builder, with networking disabled. This establishes red/green
regression evidence, not yet a passing full corrected-image launch or gameplay.
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

The public Vercel identity endpoint subsequently reported source `cd85da8`,
deployment `dpl_GxbCtQAVX3WsoFq8fCGCycxWzJ6c`, schema head
`20260907220000`, Listing `0.6.0`, and the expected Heist v3 client digest.
Fly has not yet received the corrected image. CI `34177427443` passed its
database-boundary and application-boundary jobs; the canonical local candidate
job is running. Superseded CI
`34177189544` was cancelled to avoid duplicate work.

The corrected Linux image finished building with local image ID
`sha256:7a224a56bdb4c28c641900fcdd423d38582e9bfd4c18afc812fafa45cc115fed`
and source `484d636`. All nine hosted runtime/entrypoint tests passed, including
the two image-enabled checks. Its full entrypoint reached Gateway and Runtime
readiness under one CPU and 512 MB with synthetic credentials and no network.
The same Listing `0.6.0` request that previously returned `409` now retained
one launch binding, one Room creation, and one setup operation. Setup completed;
the response remained `202 waiting_for_readiness`, with matching operation
identities, during ten explicit retryable same-operation resumes.

The diagnostic probe's immediate-lobby assertion failed and its receipts remain
failed evidence. It never connected the fixture's human and external Agent.
The actual readiness implementation requires synchronized participant sessions;
repeated launch requests alone cannot satisfy that condition. This proves the
catalog rejection is removed and Room setup proceeds, not completed gameplay.
Use the full connected-client acceptance story for the latter. Disposable local
probe containers and their fresh volumes were removed; no cloud data changed.

Both old and new image assets report the same managed House executable BLAKE3
digest, `afcc2bcd6199680559a366b729415c8323570a6d9e98dfdfe824692277925e36`.
An executable-digest change therefore does not require a new Runner Template
for this candidate.

### September 8, 01:57 UTC — corrected image deployed behind closed admission

Image `registry.fly.io/worldstream-preview@sha256:f52d73bd5799b07e45229fd06fe0668eceb77557e425c04cb64a48803863ffca`
is now installed on the same Machine `8ed004f7033e18` and retained volume.
No installation archive/reset, credential change, resource expansion, or
Runner Template change was needed. At 01:56:26 UTC, Gateway and Runtime
readiness returned `200`; Runtime and the public Gateway version endpoint
reported source `484d636`. Fly's service health check passed.

Runtime was then intentionally closed again for exact-image prelaunch capture.
Checkpoint `1e2f5b11-9a96-4f9e-87e0-0b3c089ff199` was captured at 01:57 UTC,
downloaded privately, and both archive hashes verified:

- Runtime: `sha256:2df4773da8ad8eeb9a5efb87bda395285da9d0204691b501a449c5ab99e11ed9`.
- Controller: `sha256:a3c939dd02125ed912cb653c7bc2f9319073497ad255171ab4c1ec84a1f67abd`.

Pairing this new capture with Supabase and its isolated restore verification
passed at 02:00:05 UTC. Manifest digest:
`sha256:dc39fff342288cb6b482ec9c58ac52c7b926c53db224cd4327ed01d263157891`.
Deployment document digest:
`sha256:8878661d1b54bfbead68979c9af841edfb2a8dcfaccabb31e9bd023a17c9a12d`.
The exact-image verifier confirmed matching 15-migration history, empty activity
stores, retained Host authority correspondence and closed admission. The
existing disposable local database was the only restore target; no Auth restore
or populated-history recovery is claimed. The temporary loopback database
forward was stopped after verification.

The prior verified checkpoint is retained, not relabelled as evidence for this
image. Public launch and House-fill gates remain closed; no paid provider call
has occurred. The canonical CI story remains running.

The exact deployed profiles, named credential metadata, Runner Template,
executable, and successful idempotent import receipt were captured privately.
`hosted-house-approval.mjs` prepared two receipts and separate approval/activation
SQL files. The reviewed `approve.sql` was subsequently applied through the
linked Supabase administrative query path. Readback confirmed two approvals,
zero available for new assignments, and zero revoked. `activate.sql` remains
unapplied; House Agents are not activated. The capture helper must use the appliance's explicit Controller
address and protected authority, running the CLI as UID/GID 65532; an earlier
incomplete helper invocation produced no passing capture evidence.

A fresh read-only OpenRouter key check confirmed a $2 lifetime limit, no reset,
$2 remaining, zero usage, and expiry `2026-12-06T22:29:33.475Z`. The public
endpoint catalog still lists the exact DeepInfra BF16 route at the reviewed
price ceilings. This is not a successful inference or key-specific routing
proof. Chrome still shows the signed-in owner and the current room form with
its House call/token bounds. The form was closed without creating a room.

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
