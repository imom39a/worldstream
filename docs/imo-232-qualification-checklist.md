# IMO-232 finite qualification checklist

This is the closure audit for every finite acceptance criterion in Linear
IMO-232. The 72-hour elapsed soak was split into IMO-235 and was deliberately
not run. Generated engineering reports retain `release_evidence=false`; this
document does not claim release readiness or model correctness at scale.

## Result

| # | Acceptance criterion | Result | Evidence |
| ---: | --- | --- | --- |
| 1 | Reproducible artifact, source, backend, and Linux reference resources; PostgreSQL measured separately | **Satisfied** | The [local Docker evidence directory](evidence/imo-232-local-docker-2026-09-14) binds the one-million run to source revision, three binary hashes, image content ID, Linux runtime manifest, 4 CPU limit, 8 GiB memory limit, and a disposable Docker-managed volume on the local SSD host. The [PostgreSQL report](evidence/warm-activation/imo-220-postgres-live-local.json) separately binds PostgreSQL 17.11 and PgBouncer image digests. |
| 2 | Read/claim/Action latency, useful work, stale rejection, recovery work, memory, storage growth, and pending work | **Satisfied at finite engineering scope** | SQLite reports retain read, claim/release, accepted Action and stale Action p50/p95/p99, accepted/rejected counts, oldest pending Activation, reducer calls, bounded RSS samples, context bytes, database/WAL bytes, and snapshot bytes. The one-million local Docker report uses 1,000 read and claim samples. The [isolated snapshot report](evidence/snapshot-cadence/README.md) measures one SQLite cache transaction at 737,500 ns of writer CPU, 886 logical bytes, and exactly 32,992 WAL bytes. PostgreSQL records 1,000 direct and pooled read/claim samples and keeps its unavailable exact per-snapshot server CPU and WAL fields explicit. Recovery reports expose exact row/callback counts. MMR reports expose proof-vector allocation bounds. |
| 3 | Warm operations avoid the history prefix; checkpoint recovery honestly meets the five-second reference target | **Satisfied** | SQLite warm operations continue after an in-head historical corruption while recovery is forbidden, and reducer counts remain unchanged for reads and claims. Healthy cold Gateway cache installation now uses guarded V3 recovery plus one bounded serving-fence transaction and performs zero full-history inspections. Current-source SQLite 100k and local Docker 1m executors retain zero historical Transitions. The local Docker 1m and PostgreSQL 1k/10k/100k recovery measurements are recorded in their reports. Each recovery reads one boundary Transition and delivers no prefix or tail Transitions. |
| 4 | Complete the 72-hour soak | **Moved to IMO-235** | The wall-clock termination, offline, slow-consumer, credential-renewal, and Timer-catch-up schedule remains open in the human-run [IMO-235](https://linear.app/imom39a/issue/IMO-235/run-the-72-hour-long-history-runner-soak). No shorter run is presented as equivalent. |
| 5 | Lost replies, acknowledgement durability, authorization, obligations, and resource bounds | **Satisfied for finite failure cases** | [Criterion-five evidence](evidence/imo-232-criterion-5-local.json), SHA-256 `3c5bc4bd5f4bdba6c919b7f0754588bbff1ea0d684861488bde763ff236cb2b5`, passes 16 current-source release regressions. They cover original Action/Activation/Runner identities, durable acknowledgement/revocation, restart reconciliation, Timer catch-up, access isolation, bounded queues, slow consumers, and a 10,000-arrival attention burst. Its sole skipped dimension is the IMO-235 elapsed soak. |
| 6 | Restore/transfer above 100k records and 64 MiB preserves exact state | **Satisfied** | [IMO-225 transfer evidence](evidence/imo-225-stream-closure-2026-09-14.json), SHA-256 `2dd07b1734cb37a713d943bec64833973e5e4a79baab243d36aeda77d2c14cc3`, passes 300,020 records and 785,670,211 exact bytes through 335 bounded chunks. It covers resume, corruption, disk-full, authority finalization, and exact canonical/operational equality. The final root-transfer fix additionally preserves three frozen roots, three MMR receipts, and 199,994 nodes at 100k. |
| 7 | Compare three Runner context strategies under the same model and budget | **Satisfied at bounded evaluation scope** | [Luna evaluation](evidence/runner-context-evaluation), SHA-256 `afcf043c12e45a86957bb61172b9afff412c71f409474e812d2bf0e3c07c6be5`, uses three isolated `gpt-5.6-luna` calls and a 24,576-byte ceiling. Recent history produced 2/4 useful Actions; summary plus authorized retrieval and current Projection plus explicit work each produced 4/4, with missed work and errors reported separately. |
| 8 | Keep failures and unavailable evidence visible | **Satisfied** | The local Docker, PostgreSQL, MMR, reliability, transfer, and model reports retain their evidence class and unavailable fields. They never claim `release_ready`, never convert deterministic turns into model-quality evidence, and keep the 72-hour omission visible. |

## Finite scale evidence

| Provider/path | Scale | Result |
| --- | ---: | --- |
| Production SQLite/Core fixture | 1k, 10k, 100k native; 1m local Docker | Passed |
| SQLite isolated snapshot cost | 1k local Linux; one fenced cache transaction | Passed |
| SQLite warm Gateway | 1k, 10k, 100k native; 1m local Docker; 1,000 reads and claims per tier | Passed |
| PostgreSQL 17.11 direct and PgBouncer | 1,000 read and claim samples per profile | Passed |
| PostgreSQL V3 recovery | 1k, 10k, 100k | Passed |
| Operational MMR | 1k, 10k, 100k, 1m; 1,000 proofs per tier | Passed |
| SQLite-to-PostgreSQL portability | 100,000 Transitions | Passed |
| Streaming portability | 100,001 Transitions; 300,020 records; 785,670,211 bytes | Passed |
| Runner failure/reliability matrix | 16 finite cases | Passed |
| Model context comparison | 3 strategies x 4 cases | Passed |

## Source split

The Linux one-million image and fixture were built and run locally from
`eab6d8c18187f7bc7fb8f145ab7fcaca369a71c8`. Commit
`a241bad5204162d2d2d5fc08cb61c64426b4bc70` added executable provenance, while
`40357cc351a91066a71a0dbd76be28560cf4de03` and `eab6d8c1` bounded the healthy
cold Gateway cache installation and its exact transaction/race fences. The
`b8d6044ac6faa5f647f4eac7dbd214a9ed453584` transfer delta is covered by the
final local 100k transfer report and backup/transfer suites. The isolated
snapshot-cost fixture and full 157-test SQLite Linux suite were built locally
from `a187c53c97d86bde963821d61a8a5a752eb3670d`; their temporary containers,
volume, and images were deleted after evidence capture.

## Reproduction entry points

```sh
scripts/postgres-live-evidence.sh
scripts/imo-220-warm-claim-qualification.sh
scripts/imo-220-warm-safety-matrix.sh
scripts/imo-225-stream-closure.sh
scripts/imo-232-criterion-5-evidence.py
scripts/runner-context-evaluation.py
cargo run --locked --release -p worldstream-core \
  --example operational_mmr_qualification
```

IMO-235 remains the only open item in this qualification frontier.
