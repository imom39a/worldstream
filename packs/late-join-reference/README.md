# Late Join Reference Pack

This small, nonterminal Activity Pack is a public TypeScript reference for
late joining. It models one bounded set of current connection facts and open
assessment work. A newer source revision increments the work revision and
marks its prior assessment `superseded`; a current assessment can resolve the
work only when it matches the deterministic transfer rule.

The Pack uses only `descriptor`, `initialize`, `reduce`, `view`, and `observe`.
Participant `view` returns every authorized open obligation and its Action
Offers. `observe` emits a complete `projection_replaced` view, so a client
replaces its current view instead of appending another copy to a prompt. The
public view contains only aggregate counts.

`src/runner.ts` is a provider-free external Runner example. It assembles a
versioned rule brief, current Projection, exact offered action schemas, and at
most four bounded recent outcomes. It reports encoded bytes and a conservative
four-bytes-per-token estimate, keeps one current view, replaces an Invocation
after each contribution, retries a lost reply with the original Action ID, and
fences stale proposals until a refreshed Projection is installed.

Run the conformance and continuity checks with:

```sh
pnpm --filter @worldstream/late-join-reference pack:test
```
