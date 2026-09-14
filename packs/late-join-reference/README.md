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

The reference also declares one generic Host Stimulus Source,
`worldstream.external_input.v1`, from source `01ARZ3NDEKTSV4RRFFQ69G5FH2`.
Its canonical payload is the bounded source fact shape used by
`record_source_update`. Reduction applies the same monotonic revision rules as
an analyst Action, marks the prior assessment superseded, and emits one
`assessment_required` Attention Signal. Equal source revisions cannot rewrite
their associated values, and the connection's transfer policy stays fixed.
An admitted outdated or inconsistent fact produces `source_fact_ignored`
with its reason and source provenance; it advances canonical Head while
preserving current facts. ExternalInput is mandatory after Core admission,
so a domain rejection must not reject the stimulus and fault the Room.
The ingress adapter refuses unsupported source/type, exact Pack pin mismatch,
and oversized envelopes before reduction.

Each applied source correction schedules or replaces one Room-wide reminder
Timer, due thirty seconds after the input's recorded time. A firing consumes
the exact generation and never reschedules itself. Current assessments stop
further assessment offers and Attention, including an unresolved risk that
remains visible in the Projection. A later source revision reopens assessment.
Reduction and Replay perform no network, provider call, or ambient clock read.

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

The portable acceptance tests require the production Component Host proof,
including finalized golden evidence. Use the repository's pinned Node version:

```sh
cargo build -p worldstream-server --bin worldstreamctl
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --filter @worldstream/late-join-reference pack:prove
export WORLDSTREAM_EXTERNAL_INPUT_BUNDLE="$PWD/packs/late-join-reference/.worldstream/late-join-reference.wspack"
cargo test -p worldstream-server --lib \
  external_input::acceptance::sqlite_portable_external_input_acceptance -- --ignored --nocapture
```

The local production-host proof for the 2026-09-13 reference revision is
retained in [`fixtures/portable-proof.json`](fixtures/portable-proof.json).
It binds the exact immutable Pack revision, bundle, and finalized transcript;
later builds must produce their own proof. It is not deployment release evidence.

To run the same assertions against live PostgreSQL, provision a fresh
PostgreSQL 17 database using the migration and least-privileged runtime grants
in `scripts/postgres-live-evidence.sh`, then set
`WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE` to its owner-only runtime DSN file:

```sh
cargo test -p worldstream-server --lib \
  external_input::acceptance::live_postgres_portable_external_input_acceptance -- --ignored --nocapture
```

Both tests load the same portable artifact through `PackBundleVerifierV1`,
`ComponentPackHostV1`, and `PackRegistryV1`, then use production gateway/store
APIs. They assert bounded Projection and historical Replay, source provenance,
out-of-order/correction behavior, receipt retry/restart, authority and health
fences, and concurrent Action/ExternalInput/Timer ordering. A test-only held
admission position establishes deterministic FIFO reservation order and
saturates a four-position lane; excess ingress returns Busy. These are bounded
correctness tests, not a sustained throughput or 100,000-turn performance claim.
