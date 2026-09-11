# Room-service reliability: September 10, 2026

## User-visible problem

Room creation, entry, and My games intermittently showed service-unavailable
errors. These messages do not by themselves prove that the Fly Machine stopped.

During the investigated incident (15:14:58–15:16:19 UTC), Vercel recorded 503s
for Run entry, Launch start, and My games. The same deployment continued to
return 200 for authentication, Launch details, catalog, and public results.
My games recovered at approximately 15:16:22 UTC without a new deployment.

Fly's inspected Machine events showed its planned 14:28:31 UTC deployment
start, not a later crash loop. Readiness returned 200. The inspected Machine
had about 334 MB available memory and only 17 MB used on its volume. The
retained log window was limited; this is not proof that no older error occurred.

The failing Vercel invocations contained no underlying exception details. The
original trigger could not be established from those logs. Do not describe it
as a proven Fly crash, SQL defect, timeout, or memory problem.

## Reproduced defects

1. My games first completed its authenticated, account-scoped database read,
   then awaited a global result-maintenance pass. An exception in that pass
   replaced the successful read with a 503. One unrelated candidate could make
   personal history unavailable. A deterministic test reproduced this.
2. The Vercel configuration omitted the `/my-games` SPA rewrite. Direct links
   and refreshes returned 404 even though in-app navigation worked.
3. Five-second browser interval polling could overlap requests while a slow
   maintenance pass was still running.

## Focused correction

- Keep the initial authenticated read. Anonymous callers cannot initiate this
  maintenance pass.
- After maintenance succeeds **or fails**, perform another authenticated,
  account-scoped read. Never reuse an earlier body after account revocation,
  erasure, or a database-read failure.
- If maintenance failed but that fresh private read succeeded, return its
  history with `X-WorldStream-Refresh: delayed`. The site clearly labels the
  status as delayed. History is not authority to enter a Room or infer its
  current gameplay state.
- Share only the maintenance promise/outcome within the function process,
  never account-specific response bodies. Use a ten-second successful-pass
  cooldown and a five-second failed-pass cooldown. Other function instances
  are independent; this is not a distributed lock or a queue.
- Start the next browser refresh five seconds after the previous one finishes.
  Abort and ignore an in-flight refresh when the page is left or sign-in state
  changes.
- Add the exact `/my-games` rewrite. Do not add a blanket fallback that hides
  missing client artifacts or API routes.

Room entry still resolves the signed-in account's exact Membership and asks
Fly for an authorized one-use handoff. Failures do not invent a Room, grant
access from history, silently repeat a mutation, or discard platform sign-in.
The pre-existing bounded capacity-repair retry on a specific start conflict
is unchanged.

## Diagnostics and privacy

The production BFF generates a new `X-WorldStream-Request-Id` for each function
request. Incoming IDs are not trusted. Failure logs carry that ID, a coarse
route category, operation, elapsed time, and an allowlisted error category.
Dependency errors also record a numeric upstream status or a source-controlled
RPC name and validated database error code where available.

Logs exclude request URLs/queries, account and Room identifiers, cookies,
tokens, provider keys, RPC arguments, response bodies, exception messages,
stacks, and arbitrary nested causes. A log-sink failure must not change the
response. No paid logging service or log drain is added. Runtime logs remain
subject to the hosting provider's retention limits.

To investigate a future failure:

1. Record its time, request path, HTTP status, and response request ID in the
   browser's Network panel. Do not copy cookies or authorization headers.
2. Search Vercel runtime logs for that request ID. Include HTTP 5xx requests,
   not only the provider's error-level filter.
3. Follow the reported operation: `supabase_rpc`, `fly_formation`,
   `fly_browser_session`, `fly_result_source`, or the account/entry boundary.
4. Correlate the same time window with Fly events, readiness, and logs. Do not
   restart or delete the database merely because the UI reports unavailable.

## Verification and release boundary

Local regression coverage includes maintenance failure/recovery, failure
cooldown, concurrent account scope, anonymous rejection, fresh-read revocation,
database-read failure, delayed-history display, non-overlapping polls, request
ID isolation, safe logs, and an entry timeout followed by an explicit retry for
the same Room/Membership. Existing enter/redeem/re-enter/spectator and session
continuity tests remain required.

This patch changes the Vercel BFF and site only. It needs no database reset,
schema migration, Fly resize, new Machine, billing change, or LLM call. It does
not change published v9 client bytes, Pack rules, or Room transport semantics.
It is being handed to the parallel creator-close release for one coordinated
deployment; local test results are not evidence of production acceptance.

Remaining limits:

- A maintenance pass can still delay the initial read while it runs. This
  patch handles failure and polling amplification; it does not introduce a
  background worker or promise a new latency bound.
- The original incident's first failing dependency is not proven. Use the
  new diagnostics if it recurs; do not call instrumentation alone a root fix.
- The hidden Room-head freshness issue, tracked separately in
  [IMO-217](https://linear.app/imom39a/issue/IMO-217), is not fixed by this patch.

Reference guidance: [Vercel runtime logs](https://vercel.com/docs/logs/runtime)
and [Supabase retry guidance](https://supabase.com/docs/guides/api/automatic-retries-in-supabase-js).
No new automatic retry layer is introduced here.

## Initial connection recovery follow-up (IMO-219)

At 19:17:01 UTC on September 10, Vercel logged an upstream 503 from
`fly_browser_session` while issuing a Stream Admission Ticket. Handoff
redemption had succeeded. The inspected Machine had not restarted; re-entry
to the same Room later succeeded. This proves the failing boundary, not its
internal cause. A passing readiness endpoint is not a session-path test.

The follow-up reproduces two independent user-visible defects:

- `pnpm --dir web/demos test LaunchPage.browser`: a pending 30-second read
  caused 16 reads instead of one; an unmounted request was not aborted; a late
  response could replace a newly selected Launch.
- `pnpm heist-client:test AgentHeistClientView`: an initial disconnected
  client had no Reconnect button and an invalid session had no re-entry advice.

The corrected Launch page waits two seconds **after completion** before its
next read (five seconds after failure). Leaving the page, changing the Launch
or authenticated session, or beginning a mutation aborts the local read and
ignores its eventual response. Reads pause during entry/start/close/seat
mutations. A terminal response stops polling. Cancelling a browser request
does not promise cancellation of work already running in Vercel or Fly.

The Heist awaiting screen distinguishes connecting, reconnectable, and
setup-required states. Explicit reconnect is guarded against double clicks and
uses the existing SDK's retained-session revalidation and fresh-ticket path.
It never redeems the original handoff again, automatically repeats an Action,
installs fixture state, or enables Actions before authorized synchronization.
A missing/revoked session instead directs the player back to My games for
Run Re-entry. The initial failure itself remains visible.

### Release-build diagnostics

The Hosted Gateway records fixed browser operation labels, numeric Controller
HTTP status (or transport failure), and elapsed time. The Controller emits
`hosted_session_diagnostic` events with a process-local call counter, PID,
operation, stage, detail, closed category, numeric upstream status and duration.
Stages separate authority resolution, Membership lookup, Client Selection,
Runtime ticket issuance, and overall session completion. Nested Runtime HTTP
probes distinguish connect/write/read failures, timeouts, HTTP status,
malformed responses, and validation failures.

These scopes run inside synchronous broker calls on their existing blocking
threads; they must not be carried over an async suspension or moved to another
thread without explicit propagation. Scopes restore on return and unwinding.
The Gateway and Vercel IDs are not a shared distributed trace: use timestamp
and operation to find the Controller call, then PID/call to group its stages.
Only authenticated session operations produce Controller events. This is
bounded per-operation logging, not a packet/body dump, a new paid log drain,
or per-frame logging. No arbitrary exception text, query, body, token, origin,
account, Room, Membership, or capability enters the event schema. Sink write
errors do not change admission results.

Important activation gap: the detached Controller launcher currently sets
stderr to `Stdio::null()` in `operator_connection.rs`. The new Controller events
are tested, but **are not yet observable in the packaged Fly service**. Gateway
tracing already inherits the appliance log stream. The coordinated appliance
release must connect the Controller diagnostics to a bounded protected log file
or an explicit appliance-owned log sink, then verify an actual event. Do not
inherit the startup CLI's captured pipe: a long-lived child can keep the Node
startup command waiting for its `close` event. Do not call the deep diagnostics
operational until this wiring is tested.

After that log wiring is verified, inspect the session path with:

```sh
fly logs -a worldstream-preview --no-tail | rg 'hosted_session_diagnostic|hosted browser Controller request'
```

Use the failure time and operation from the Vercel request to locate the
corresponding Controller call. Within that call, inspect the first failed
stage, not only the final `session/unavailable` summary. A nested `read/timeout`
or numeric non-2xx HTTP result points to a different boundary than an
`authority` or `client` rejection. If the Gateway records a transport failure
but no Controller call appears, inspect Controller reachability and its worker
availability next. Absence of a log is not proof that a request never arrived;
check the retained log window and process start time too. Do not include session
cookies, handoffs, tickets, or raw response bodies in a bug report.

### Successor and deployment gate

Client v11 is the current immutable successor. It adds retryable `room_busy`
synchronization within the attempt deadline; v10 remains retained:

- Release: `sha256:40d452a04b096d3a0952f99b877f094f90c6e4a84063d3212265b5c2b97306ee`.
- Build: `sha256:ce7c86d24832158536df90384d140ae5aec6482eb654ab70c1551effc9e62093`.
- Listing 0.28: `blake3:d738402a5acb404dead979c21002fa02d95f4c1e89f6d4c47e617a6c6be27bc3`.
- Existing v10/v9 bytes, Listings 0.27/0.26, Pack 0.5, projectors, House identities and
  budgets remain unchanged. New bindings are eligible explicit choices; they
  do not overwrite an existing Host default.

`supabase/catalog/agent-heist-0.28.0.sql` inserts only the exact immutable
Listing metadata. It is not a schema migration and grants no Host approval.
The successor seed uses the same exact-document conflict check as its predecessor.
Local seed configuration and `hosted:dev` include this data-only seed. This
follows [Supabase's separation of seed data from schema changes](https://supabase.com/docs/guides/local-development/seeding-your-database).

Production activation must be coordinated with the active appliance release:

1. Finish native regression checks and the appliance smoke on a clean combined
   commit. Do not deploy either partially tested candidate.
2. Preserve current operational history. No reset is needed for this change.
3. Package v11 plus retained v10/v9/v8/v7 releases in the Fly image. Import their
   exact approved client declarations. Add Listing 0.28 to the allowlist while
   retaining previously admitted Listings.
4. Apply the one data-only catalog file with stop-on-error; verify its exact
   digest/document and client correspondence.
5. Deploy the combined source-built Fly and Vercel candidate under the existing
   maintenance process. Keep the one-Machine and spending limits unchanged.
6. Connect and verify the Controller diagnostic sink described above. Check an
   authorized enter/redeem/ticket/stream flow and confirm both the Gateway and
   Controller diagnostic records. Exercise retry in a fault-controlled local browser;
   do not deliberately break production or spend LLM credits for that test.
7. Record actual production deployment identities and acceptance separately.

The browser regression drives the real built v11 client with an injected
initial ticket 503. It checks one handoff redemption, retained-session retry,
one fresh ticket request, no Action or fixture state, and re-entry guidance
after session expiry. This is not proof that the original intermittent Fly
503 has been fixed. Production activation and deeper root-cause diagnosis
remain separate gates until verified.

Local verification on September 10:

- Demos: 46 tests; Heist Client: 57; retained-session SDK: 15.
- Hosted platform: 178 passed, one integration-gated test skipped.
- Packaging/workspace checks: 71 passed, two Linux-image-only checks skipped.
- Native session diagnostics: two tests; hosted browser broker: ten tests;
  participant handoff: 23 tests; Gateway boundary: 31 tests. The handoff fixture also checks real HTTP
  rate-limit and timeout events without leaking its synthetic authority or body.
- Real Chromium recovery checks, including phone-width overflow checks, passed.
  SDK tests separately prove successful authorized Reset/synchronization after retry.
- Current and retained client artifact verification, TypeScript checks, and the
  data-only seed idempotence/rollback check passed.
- The broad warnings-as-errors Clippy pass stopped at a pre-existing
  `clippy::map_unwrap_or` warning in unchanged
  `crates/worldstream-core/src/agent_heist_lobby_v5.rs:164`. This patch does not
  change gameplay code to clear that unrelated warning.

These are local results. The coordinated source-built appliance and authorized
production acceptance remain deployment gates; none of the above claims to
reproduce or eliminate the original intermittent Fly 503.
