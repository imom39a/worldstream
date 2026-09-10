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
