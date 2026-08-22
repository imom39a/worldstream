# IMO-61 final acceptance report

## Disposition

This is a factual parent-orchestrator evidence report for the current dirty
checkout, not a release approval. The implementation work is substantially
advanced, but the repository is not ready to claim v0.1 acceptance or mark the
remaining Linear issues Done.

The continuation addendum at the end supersedes earlier numeric test counts
and lane summaries in this report where they differ.

The compatibility manifest remains fail-closed:

- `manifest_kind = "specification"`
- `release_ready = false`
- PostgreSQL 17.11 patch evidence and the direct/transaction-pooler evidence
  row are now resolved with a dated local artifact digest;
- SQLite bundled-engine digest, SQLite migration checksums, Agent Heist pack
  digests, release artifact digests, and the other evidence artifact digests
  remain unresolved.

## Parent-verified implementation lanes

| Lane | Current evidence and boundary |
|---|---|
| Core/domain, protocol, authority, replay, Activity Pack, Agent Heist privacy | `cargo test --workspace --locked` passes all 325 workspace tests; the suites cover typed authority, fail-closed pack selection, canonical hashing, replay, action/timer admission, privacy, native backup/restore, transfer fencing, telemetry bounds, and absent-runner behavior. |
| Bundled SQLite | The 75-test adapter matrix passes atomic failpoints, contention, receipts, authority, timers, delivery, snapshots, recovery, corruption/quarantine, gateway capability lookup/barriers, and replay. This is not native crash/power-loss evidence. |
| SQLite migrations | The parent-verified 75-test SQLite suite includes atomic migration failure/restart, exclusive writer ownership/release, gapped/downgraded/future/mismatched ledger rejection, and verified bundled online-backup publication before pending migration work. Process-kill and power-loss durability remain separate gaps. |
| Native SQLite evidence | `worldstream-backup` passes 18 tests, including bundled SQLite 3.53.4 read-only verification, WAL/active-writer locking behavior, real online-backup/restore publication, atomic no-replace output, interrupted-transfer cleanup, destination-race preservation, lineage/materialization hash checks, and disposable absent/corrupt paired snapshots. Disk-full, crash/power-loss, and full operational-table semantic restore remain unproven. |
| SQLite-to-PostgreSQL transfer contract | `worldstream-transfer` passes 14 tests, including deterministic bounded file export/import, canonical record ordering, deployment/Room byte parity, source/target backend fingerprints, resumable idempotent destination publication, verification-gated finalization, abort cleanup, and epoch fencing. The PostgreSQL destination stages exact canonical chunks transactionally, but no live adapter-to-adapter import, Core-table hydration, or provider-backed full deployment finalization is proven. |
| PostgreSQL | Fixture tests and a live PostgreSQL 17.11 + PgBouncer transaction-pool run pass direct-admin migration/schema verification, least-privilege runtime DDL denial, direct create/duplicate/conflict/resolve, and pooled duplicate/resolve. The live evidence digest is recorded in `compatibility.toml` and points to `docs/agents/imo-44-50-live-postgres-followup.md`. Remote TLS runtime is not claimed because the adapter uses `NoTls`. |
| Protocol/Python gateway | Versioned protocol/server tests pass (12 protocol and 22 server tests); the gateway enforces capability/action bounds, bounded sync bursts, explicit sync barriers, safe ObservationAck ownership, strict `wsb1:` bearer decoding, and transport-session binding. SQLite now durably authenticates active capabilities, serves verified current projections, maps authorized Observation ACKs, and translates attach/sync through Core's opaque session token. Python SDK checks pass (17 SDK tests), with reconnect/reset/duplicate/stale/privacy fixtures. Create/action, live WebSocket, and real restart evidence remain unproven. |
| Python/UI/toolchain | Pinned Node 24.18.1, pnpm 11.19.0, and uv 0.12.5 are provisioned. Python SDK and story checks pass (24 tests total, Ruff format/lint); UI lint passes, 10 UI tests pass, and the UI build passes. |
| Heist/story-console parity | The shared `examples/heist/parity_fixture.json` is validated by the Python story and UI fixture; 7 Heist/story and 10 UI tests pass, including six-phase/privacy/replay digest parity, strict transport request validation, and a fresh-checkout offline runner. Real process kill, power loss, and browser/server transport remain unproven. |
| Telemetry and operator shell | The 20 server tests and operator-shell smoke pass for redaction, W3C traceparent, Prometheus formatting, bounded nonblocking queue/batches, exporter outage/panic/slow handling, readiness classification, and bounded shutdown. |
| Offline absent-Broker story | The deterministic Heist self-test and 5 Python story tests pass. This is an offline fixture, not live browser/server/power-loss evidence. |
| Packaging mechanics | Package smoke passes reproducibility, checksum metadata, archive traversal, secret-input, symlink, duplicate-TAR, and release-path checks. It does not create a release artifact. |

## Exact parent gate results

Commands were run from `/Users/vinothshanmugam/code/agent-streamer`.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo test --workspace --locked` | pass; 325 tests, 0 failures; doc tests also pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `python3 tests/package_smoke.py` | pass |
| `scripts/gates.sh pre-push --offline` | exit 1; 54 checks, 10 unresolved contract failures, 1 incomplete optional check; every executable local evidence row passes, while live/platform/failure contract rows remain unresolved |
| `scripts/gates.sh release --offline` | exit 1; 50 checks, 13 failures, no incomplete skips; fails closed on manifest state, unresolved contract evidence, and missing release artifacts |
| live PostgreSQL test with direct and pooler DSNs | pass; PostgreSQL 17.11, direct admin/runtime, and PgBouncer `pool_mode=transaction` |

The pre-push contract failures are intentionally visible for migration,
cross-backend transfer, native restore, snapshot/kill-point durability,
privacy/capability/lease/filesystem/telemetry evidence. The one incomplete
check is the optional `WORLDSTREAM_POSTGRES_URL` gate cell; gitleaks and
cargo-audit both pass. The separate live harness is stronger evidence for the
PostgreSQL row and is recorded independently. Local evidence rows now count
the 4 migration, 14 transfer, 18 native backup/restore, 5 snapshot, privacy,
capability, lease, telemetry, and filesystem probes without promoting them to
live or release evidence.

## Explicit remaining evidence

| Area | Not yet proven |
|---|---|
| Native backup/restore | Disk-full, crash/power-loss, native PostgreSQL snapshot/PITR/dump/isolated restore, live adapter integration, and full semantic verification across every receipt/timer/frame/Activation/context row. WAL/active-writer/read-only and interrupted native transfer behavior are now locally covered. |
| Full transfer | Real SQLite export to an empty PostgreSQL target, live canonical-byte parity, provider-backed durable target checkpoints/finalization, nonterminal Activation fencing, and post-write rollback policy. |
| Failure and performance | Process kill, power loss, filesystem fault, failure/fuzz/resource campaign, one-hour SQLite soak, and p50/p95/p99 reference benchmarks. |
| Security/telemetry depth | A real remote TLS PostgreSQL connector, hosted/native Windows and Linux release cells, and the full cross-process collector/daemon path. |
| Release | `release_ready` remains false; native archives, OCI image, source archive, checksums, Sigstore bundle, SPDX SBOM, SLSA provenance, and all required digests are absent/unresolved. Local gitleaks and cargo-audit scans now pass. |

These are substantive acceptance gaps, not toolchain excuses. The Linear
issues remain In Progress until their own acceptance criteria have evidence.

## Orchestrator continuation (2026-08-20)

The parent reviewed and integrated five new disjoint Luna lanes through cmux,
then verified their changes in the shared checkout:

- The optional server telemetry handle is now wired into HTTP Room creation
  and WebSocket hello/action/frame post-result paths. Export failure remains
  isolated from authoritative route outcomes.
- SQLite gateway Room creation now uses the Core-authorized Genesis path. A
  parent integration test proves first create, exact idempotent replay, and
  same-key semantic conflict; participant Action remains explicitly
  fail-closed because no safe public mutable Core actor-trace seam exists.
- Native SQLite backup evidence now has 21 passing tests. The PostgreSQL
  crate has 15 passing tests, structural Room verification, and the target
  fence migration. No live PostgreSQL DSN evidence is claimed in this
  continuation.
- `scripts/restart-smoke.sh` and `tests/restart_smoke.py` provide a
  deterministic process/restart/readiness harness; its three Python tests
  pass. A live Linux run is still required, and the current daemon reports
  `storage_not_initialized` from `/readyz`, so live Room creation is not
  claimed.

Current parent verification passes `cargo fmt --all -- --check`,
`cargo test --workspace --locked` (332 Rust tests), workspace clippy with
`-D warnings`, `git diff --check`, restart-smoke tests, package smoke, and
gate smoke. The final offline gate runs are also explicit: pre-push reports
50 checks, 11 failures, and 6 incomplete skips; release reports 46 checks and
19 failures with no incomplete skips. They fail closed on unresolved
live/platform/failure evidence, unavailable current `node`/`pnpm`/`uv`
tooling, manifest readiness, and absent release artifacts. The Linear issues
therefore remain In Progress.

## Orchestrator follow-up (2026-08-20)

Seven disjoint Luna lanes were reviewed and integrated by the parent
orchestrator. The changes strengthen fail-closed boundaries; they do not
convert fixture or provider-neutral evidence into release evidence.

- Protocol/server: `wsb1:` bearer wire encoding is strict and redacted,
  `ServerWelcome.session_id` is canonical and transport-bound, and every
  `GatewayBackend` call receives the authenticated session context.
- Native backup: destination races, interrupted-copy cleanup, source/target
  non-replacement, and actionable redacted diagnostics are covered through the
  real bundled SQLite atomic publication path.
- Transfer/PostgreSQL: deterministic target fingerprints and exact canonical
  chunk bytes are staged through a transactional PostgreSQL destination with
  resume, overlap rejection, verification, finalization, and rollback fences.
  This is not a claim of SQLite extraction, Core-table hydration, or live DSN
  evidence.
- Python/Heist: strict canonical bearer/envelope/body validation, membership
  isolation, sync-barrier handling, retry identity, and transcript validation
  pass without changing the reviewed Heist digest.
- SQLite gateway/daemon: active capability authentication, verified current
  projection, authorized Observation ACK mapping, attach/reset-or-retained
  delivery, Core-backed session-bound sync acknowledgement, bounded session
  binding, and `sqlite-bundled` daemon startup are wired. Create and Action
  remain fail-closed until their Core seams are available.
- Console transport: strict bounded envelope/request validation and a
  controllable in-memory transport seam pass while the UI remains explicitly
  fixture-backed.

The parent reran the workspace, package, SDK, Heist, UI, compatibility, and
gate commands after integration: 325 Rust tests passed, 24 Python tests
passed, 10 UI tests passed, Ruff/format/lint passed, and `cargo xtask compat
verify` passed. `pre-push --offline` remains fail-closed at 10 unresolved
contract rows plus one optional live-Postgres skip; `release --offline`
remains fail-closed at 13 failures because release readiness, required
artifact digests, unresolved contract rows, and the release manifest are still
absent. The issues therefore remain In Progress.

## Parent orchestrator verification addendum (2026-08-20, current)

This addendum is the authoritative current evidence for the dirty checkout.
The full workspace run now passes 357 Rust tests (including 21 backup, 142
Core/Heist, 16 PostgreSQL, 12 protocol, 30 runtime, 34 server, 77 SQLite,
17 transfer, and 8 `xtask` tests), with formatting, strict Clippy, and
`git diff --check` passing.

Additional parent-owned evidence:

- A fresh PostgreSQL 17.11 container plus transaction-mode PgBouncer passed
  direct-admin migration/schema verification, runtime DDL denial, direct
  create/duplicate/conflict/resolve, direct advance/frame/semantic-receipt
  persistence, and pooled duplicate/resolve.
- The fuzz-gated bounded soak runner passes 507 complete 77/77 SQLite matrix
  iterations over a 3,600.018-second one-hour window, including the
  deterministic replay/property probe, resource/fault/corruption hooks,
  exact test-count validation, measured peak RSS of 131,022,848 bytes, and
  p50/p95/p99 command times of 7,112/7,527/8,028 ms. A separate real Linux
  daemon harness proves the committed Counter-v2 Room outcome survives
  SIGKILL and same-directory restart; physical power loss, disk-full, and
  filesystem fault remain explicitly unmeasured.
- Linux reference verification in `rust:1.97.1-bookworm` passed the full
  workspace suite and a real daemon smoke: `/healthz` 200, `/readyz` 503 with
  typed `storage_not_initialized`, and `/version` with verified SQLite engine
  identity. The same pinned container also passed `scripts/restart-smoke.sh`:
  the SQLite file survived termination/restart, readiness stayed fail-closed,
  and invalid storage did not serve.
- Python SDK/story checks pass (24 tests plus Ruff), the UI has 10 tests,
  TypeScript, and a production Vite build passing, and package smoke passes
  deterministic Linux/source/OCI dry-run and fail-closed negative fixtures.
- Operator-shell smoke now matches the verified post-start `/version` contract
  and passes.

The refreshed offline disposition is: `pre-push --offline` exits 1 with 50
checks, 11 failures, and 6 incomplete skips; `release --offline` exits 1 with
46 checks, 19 failures, and no incomplete skips. Remaining failures are the
pinned host tool cells, unresolved migration/transfer/restore/snapshot/
failure-campaign/privacy/capability/lease/filesystem/telemetry evidence rows,
`release_ready=false`, missing release artifacts/signatures/SBOM/provenance,
and absent Windows/remote-TLS/full cross-backend/power-loss
evidence. No Linear issue is marked Done while those acceptance requirements
remain unproven.

## Parent continuation addendum (2026-08-20)

The final Luna telemetry lane was independently verified after this report was
written. The real `POST /v1/rooms` router test now proves post-result
admission-before-commit telemetry ordering, exactly two enqueued events, zero
drops, bounded 4 KiB encoding, redaction, bounded shutdown, and exporter
failure isolation; the server library suite is 32/32 with formatting, Clippy,
and `git diff --check` passing.

An explicit `--one-hour` soak probe was started in cmux with the available
Python 3.13.0 runtime after the pinned 3.14.7 invocation failed closed. It was
stopped before the 3,600-second window completed so the checkout would not
retain a long-running child; no one-hour evidence artifact was produced. The
first attempt exposed a terminal-window timeout boundary in the harness and
was retained separately as a failed boundary artifact. After the parent fixed
that boundary, a corrected cmux run completed with `status: "pass"`,
`one_hour_window_completed: true`, 507 matrix iterations, and the full
3,600.018-second elapsed window. The artifact SHA-256 was
`46f1c6e24fa3c035a2c976855e30ed423f8511d712cbb660a6722c830306a950`.
The artifact used no configured database path (`database.status:
not_configured`), so database/WAL growth was not measured. Physical-power-loss
and disk-full portions of the acceptance row remain unresolved; the process-kill portion is covered by the Linux
`evidence_class: "process_level"` result documented in
`imo-60-61-kill-points.md`.

## Parent continuation addendum (2026-08-21, authoritative)

The parent used cmux workspace `workspace:26` to dispatch and review three
additional Luna lanes. All three workers were closed after parent verification:

- daemon telemetry is now installed by `worldstreamd` as a bounded structured-log
  exporter with runtime-lifetime shutdown/flush ownership;
- the console has an opt-in live session controller for welcome, attach, reset,
  retained delivery, sync/observation acknowledgements, action receipts, and
  redacted errors while retaining fixture mode as the default;
- PostgreSQL has typed Activation offer/claim/renew/release/complete operations
  with transactional locks, request-hash idempotency, lease generation/expiry
  fences, receipt/context hash validation, and provider-neutral lifecycle tests.

The parent also fixed the live-session ULID/handshake race, fenced live action
submission until attach, checked PostgreSQL Activation update row counts, and
verified stored Activation receipt/context hashes before accepting a replay.

Current verification from `/Users/vinothshanmugam/code/agent-streamer`:

- `cargo test --locked --workspace`: pass, including all workspace suites and
  doctests;
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`:
  pass;
- `cargo fmt --all -- --check` and `git diff --check`: pass;
- console: 19 Vitest tests, TypeScript check, and Vite build: pass;
- `python3 tests/package_smoke.py`: pass, with intentional unsafe-path and
  signer-prerequisite diagnostics preserved;
- disposable PostgreSQL 17.11 direct/runtime plus transaction-pool PgBouncer
  receipt conformance: pass. This does not claim a live Activation-specific
  cell.

The refreshed gate boundary is deliberately fail-closed: pre-push is 52 checks
with 12 failures and 6 incomplete skips; release is 48 checks with 21 failures
and no incomplete skips. `release_ready` remains false. The remaining failures
are unresolved release evidence (full migration/crash, transfer parity,
backend-native restore, platform profiles, security/observability, signatures/
SBOM/provenance, and long-duration failure/fuzz/soak evidence), plus the
unavailable pinned Node 24.18.1/pnpm/uv cells. All 15 active project issues
remain In Progress; the parent updated the project and affected issue comments
with this boundary and did not fabricate acceptance artifacts or mark an issue
Done.

## Parent continuation addendum (2026-08-21, Wave 3/4 verification)

The parent orchestrator dispatched four disjoint Luna lanes through cmux
workspace `workspace:26`, reviewed their actual checkout changes, reran their
checks, and closed all four workers:

- Python SDK retained/reset sync installation, acknowledgement barriers,
  reconnect cursor handling, pruning/reset recovery, action identity retries,
  and HTTP projection/replay boundaries.
- PostgreSQL live conformance coverage for direct runtime identity contention,
  stale-head reprepare, rollback, unknown-after-commit resolution, authority
  fencing, and normalized evidence output.
- SQLite-to-PostgreSQL transfer smoke coverage for exact canonical bytes,
  checkpoint/resume/replay, finalization, target authority, epoch fencing,
  redaction, and fail-closed missing-provider behavior.
- Native SQLite restore smoke, bounded soak/resource evidence, and the Heist
  privacy/Attention/activation/story projection corpus.

Parent verification and fixes:

- `uv run --project sdk/python pytest -q`: 35 passed; Ruff lint and format
  checks pass for the full Python tree.
- `cargo test --locked --workspace`, strict workspace Clippy, Rust formatting,
  and `git diff --check`: pass.
- Console: 19 Vitest tests, TypeScript check, and production Vite build pass.
  The UI parity assertion now uses the canonical Heist transcript digest
  `sha256:b3480b11d8aff29549dc7cc19c705e147e2f150afc22b44ce23b0bb7dca1b1cc`.
- Focused Wave 4 Python checks pass: native restore 4 tests, soak 8 tests,
  transfer 11 tests, and Heist 10 tests. The smoke runners continue to emit
  `release_evidence: false`; no fresh one-hour soak or live transfer claim was
  made by these lanes.
- A fresh disposable PostgreSQL 17.11 container with separated admin/runtime
  roles passed the direct live adapter lane: admin migration/schema
  verification, least-privilege DDL denial, create/duplicate/conflict,
  guarded receipts, same-identity concurrency, stale-head reprepare,
  rollback, unknown-after-commit guarded resolution, and authority fencing.
  The parent fixed the test fixture's durable frame-head witness and fixed the
  adapter failpoint matcher so `UnknownAfterCommit` cannot be consumed by the
  rollback check. The container and credentials were removed after the run.
- The external PostgreSQL harness then passed direct admin/runtime migration,
  forward-prefix, credential, adapter, and runtime least-privilege checks; it
  remained `incomplete` only because no transaction-pooler DSN was supplied.

Current gate boundary after this wave:

- `scripts/gates.sh pre-push --offline`: 56 checks, 11 release/contract
  failures, and 1 incomplete optional PostgreSQL-URL skip. All local code,
  toolchain, security-scan, workspace, SDK, UI, SQLite, and local evidence
  checks pass.
- `scripts/gates.sh release --offline`: 52 checks, 15 failures, no incomplete
  skips. It fails closed on `release_ready=false`, unresolved required evidence
  and artifact digests, and the absent release directory/artifacts.

The 15 project issues remain In Progress. The current evidence closes several
implementation and local-verification gaps, but does not prove the remaining
cross-backend live transfer, full native isolated restore semantic verifier,
Windows/Linux/OCI release profiles, remote-TLS and cross-process security/
observability cells, signatures/SBOM/provenance, or the full failure/fuzz/
power-loss/one-hour acceptance matrix. No release flag or Linear issue state
was changed.

## Parent continuation addendum (2026-08-21, Wave 5 and final local gates)

The parent orchestrator used cmux workspace `workspace:26` and dispatched four
disjoint Luna lanes, then reviewed the checkout changes and closed every worker:

- Zeno the 2nd: paired SQLite snapshot verification/cache fallback,
  corruption quarantine, and recovery-generation fencing for IMO-48;
- Hilbert the 2nd: PostgreSQL migration-ledger, root-initialization,
  SQLSTATE/receipt normalization, and direct conformance harness coverage for
  IMO-44/49/50;
- Maxwell the 2nd: exact-byte SQLite-to-PostgreSQL transfer and backend-native
  restore/verifier smoke boundaries for IMO-51/52;
- Bacon the 2nd: six-phase Heist reducer/privacy/Attention and absent-Broker
  story coverage, Python SDK protocol/reconnect behavior, and first-party UI
  live-session/fixture coverage for IMO-53/54/55/56/57.

Parent verification of the focused lanes passed: SQLite `84` tests; PostgreSQL
conformance `17` tests plus `19` unit tests, `13` integration tests, and `6`
harness tests; transfer `21` tests plus backup `43` tests and `15` smoke tests;
Heist core `17` tests, story `11` tests, Python SDK `37` tests, and console
`19` tests. The parent also ran the full locked Rust workspace suite and
strict workspace Clippy with all targets/features; both passed. Rust formatting
and `git diff --check` passed after a test-only `too_many_lines` allowance was
added to the newly expanded Heist privacy test.

The parent fixed the Darwin filesystem probe in
`scripts/macos-source-quickstart.sh` (GNU `df -T` was not valid on macOS),
added a native `macos-15` source quickstart job to
`.github/workflows/compatibility-gates.yml`, and verified the quickstart on
the current APFS host. The project Apache-2.0 license is now present at
`licenses/LICENSE-APACHE-2.0.txt`; OCI dry-run no longer reports a missing
license and still correctly refuses to claim a release without release
artifacts.

The final local gate boundary is deliberately unchanged in policy:

- `scripts/gates.sh pre-push --offline`: `56` checks, `11` release/contract
  failures, and `1` visible incomplete optional PostgreSQL-URL skip. All
  implementation, toolchain, lockfile, security, workspace, SDK, UI, SQLite,
  operator-shell, and local-evidence checks pass.
- `scripts/gates.sh release --offline`: `52` checks, `15` failures, and no
  incomplete skips. The failures are `release_ready=false`, unresolved
  required manifest/evidence/artifact digests, the absent release directory,
  and the external platform/signature/SBOM/provenance/failure evidence rows.

The checked-in specification manifest remains `manifest_kind = "specification"`
and `release_ready = false`. No release artifact, signature, SBOM, provenance,
cross-platform native result, transaction-pooler result beyond the recorded
PostgreSQL evidence, live transfer/restore evidence, physical-power-loss
evidence, or fully configured one-hour database soak was fabricated. The
remaining Linear issues therefore stay In Progress pending those external
acceptance inputs; this addendum is the authoritative parent boundary for the
Wave 5 verification.

## Parent continuation addendum (2026-08-21, Waves 6/7 and final parent gates)

The parent orchestrator used cmux workspace `workspace:26` to dispatch five
additional disjoint Luna lanes, independently reviewed each returned result,
and closed every worker. The fifth lane was intentionally classified as
incomplete rather than promoted into release evidence.

### Verified implementation and evidence lanes

- Hubble the 2nd (`IMO-44/49/50`) ran PostgreSQL `17.11` with separate direct
  admin/runtime roles and disposable PgBouncer transaction pooling. The fresh
  adapter run passed direct migration/verification, runtime DDL denial,
  create/duplicate/conflict/resolve, stale-head reprepare, rollback and
  unknown-after-commit guarded resolution, authority fencing, and pooled
  duplicate/resolve. The normalized SQLSTATE probes classified lock timeout
  (`55P03`), deadlock (`40P01`), and serialization (`40001`). The harness ran
  six tests and the PostgreSQL conformance suite ran 17 tests; the report
  recorded evidence SHA-256
  `c4819dc2accd595f720534cf05c27abd53b476faa900499d861d146ac2c5fc4a`.
  This resolves the provider lane only; it does not prove the remaining
  cross-backend, crash, or release-artifact criteria.
- Ohm the 2nd (`IMO-59/60/61`) added a read-only source/manifest identity audit
  and four tests. Parent review found one stale assertion after the migration
  manifest was expanded; the parent corrected it and reran the audit. The
  manifest now reports six SQLite migration bodies and seven PostgreSQL
  descriptors, with all implementation-verified migration checksums present,
  while retaining `manifest_kind = "specification"` and `release_ready =
  false`.
- Helmholtz the 2nd (`IMO-49/60/61`) added the typed six-entry SQLite
  `MigrationDescriptor` inventory and descriptor-driven contiguous-prefix
  verification. SQLite tests passed with 86 tests; strict SQLite Clippy,
  formatting, manifest parity, and `scripts/verify-manifest.py` passed. The
  existing SQLite migration ledger has no checksum column, so the lane did
  not rewrite canonical schema history or falsely claim persisted checksum
  verification; that stronger property remains a separately reviewed future
  migration.
- Dirac the 2nd (`IMO-53/54/55/56/57`) added the validated combined Counter
  v1/v2 plus exact Agent Heist registry, an audited host-authenticated
  `POST /v1/operator/member-capabilities` route, one-time plaintext bearer
  return with hash-only persistence, and SDK empty-stream cursor handling.
  Parent verification passed core registry tests, the 37-test server suite,
  strict workspace Clippy, the full 39-test Python suite, and the updated
  real-process runner. On a rebuilt daemon, the runner passed health before
  and after restart, Counter room creation and restart idempotency, capability
  issuance, member WebSocket welcome/attach, sync, observation ack, and a
  second post-restart attach/sync/ack. No bearer or secret was emitted.
  The daemon now starts its bounded scheduler and reports `scheduler = running`
  with `/readyz = 200`; this is still not a full Agent Heist execution claim,
  and the SDK action-reply timeout remains an explicit compatibility gap.
- Tesla the 2nd's earlier real-process story and fixture report remain
  authoritative for the boundary before capability issuance: the fixture
  absent-Broker story passed with replay verification, while the first live
  attach was correctly forbidden. The new capability route closes that
  specific local Counter member-path gap without claiming scheduler-backed
  Heist completion.

### Incomplete release lane

Einstein the 2nd (`IMO-59/60/61`) produced useful but non-promotable evidence:
the Linux `rust:1.97.1-bookworm` workspace build and process-level kill-point
restart probe passed, and the disposable OCI policy checks were exercised.
The native Linux gate did not complete, strict Clippy in that copied run
reported documentation failures, macOS correctly refused Linux packaging,
and no persistent report or release artifact was produced. Parent current
workspace Clippy is green, but this lane does not resolve native Linux/OCI
release acceptance.

### Parent-owned corrections and final gates

The parent updated `examples/heist/wave6-live/run_live_story.py` to exercise
the audited capability route, made its blocking HTTP calls async-safe, fixed
its ULID idempotency key, and ran Ruff formatting. The parent also updated the
manifest identity audit's stale PostgreSQL expectation. The authoritative
local gates now report:

- `cargo test --locked --workspace`: pass;
- `cargo clippy --locked --workspace --all-targets --all-features -- -D
  warnings`: pass;
- Rust formatting and `git diff --check`: pass;
- Python SDK: 39 tests, Ruff format/lint: pass;
- console: 19 tests, TypeScript check, and Vite build: pass;
- manifest audit: 10 tests, `scripts/verify-manifest.py`: pass;
- PostgreSQL direct/pooler conformance: 17 tests, harness: six tests, pass;
- `scripts/gates.sh pre-push --offline` with Node `24.18.1` and pnpm
  `11.19.0`: 56 checks, 11 release/contract failures, one visible optional
  PostgreSQL-URL skip; all implementation, toolchain, lockfile, security,
  workspace, SDK, UI, SQLite, operator-shell, and local-evidence checks pass;
- `scripts/gates.sh release --offline` with the pinned toolchain: 52 checks,
  15 failures, no incomplete skips. It fails closed on `release_ready=false`,
  unresolved required/evidence/artifact digests, the absent release
  directory, and the external platform/signature/SBOM/provenance/failure
  evidence rows.

No release flag was flipped, no fabricated artifact was added, and no Linear
issue was marked Done. All 15 active project issues remain In Progress pending
the unproven external acceptance rows, including full Heist scheduler/runtime
execution, native Linux/Windows/OCI release artifacts, live transfer/restore,
crash/power-loss and long-duration database evidence, remote-TLS and
cross-process security/observability, and signatures/SBOM/provenance.

## Parent continuation addendum (2026-08-21, Wave 9 Runner boundary)

The parent orchestrator used cmux workspace `workspace:26`, closed the
completed Carson Luna SDK lane, and dispatched one final disjoint Luna lane for
the absent-Broker live harness. The parent retained integration authority and
did not promote a worker's report without independent verification.

Wave 9 closed the concrete scheduler/Runner transport gap without changing the
release contract:

- the daemon now owns a bounded Activation scheduler runtime, performs a
  startup tick before readiness, and logs `scheduler = "running"`;
- Runner mode is accepted only on `/v1/runner/stream`, while the Room endpoint
  remains `/v1/stream`;
- `POST /v1/operator/runner-capabilities` installs the typed Agent principal,
  Runner, and bounded Runner-control Capability chain for an exact Agent
  Membership allowlist, returning a one-time cache-disabled bearer;
- the server route/authority regression passed with the new server suite:
  39 library tests and 2 daemon tests;
- a disposable real daemon, actual issued Runner bearer, and actual Python SDK
  proved `readyz=200`, `/v1/runner/stream`, correlated `runner.ready`, and an
  authorized empty `activation.offers` poll after restart-safe startup;
- the updated Counter live story proved `readyz=200` before and after restart,
  HTTP create/idempotency, member capability issuance, attach/sync/ack, and
  `Room.act` correlation. It remains `partial` because it is not a full Heist
  execution;
- Carson's SDK lane passed 33 SDK tests, 11 Heist example tests, Ruff checks,
  and format checks. `examples/heist/run_runner.py` remains fail-closed and
  makes no live acceptance claim;
- `scripts/smoke-operator.sh` was updated to match the now-running scheduler;
  operator-shell smoke passes;
- with the pinned Node 24.18.1/pnpm 11.19.0 toolchain,
  `scripts/gates.sh pre-push --offline` reports 56 checks, 11 intentional
  release/contract failures, and one optional PostgreSQL-URL skip; all code,
  toolchain, security, workspace, SDK, UI, SQLite, operator-shell, and local
  evidence checks pass;
- `scripts/gates.sh release --offline` reports 52 checks and 15 failures,
  still fail-closed on `release_ready=false`, unresolved required manifest and
  evidence/artifact digests, missing `dist`, and external platform/signature/
  SBOM/provenance/failure evidence;
- `python3 tests/package_smoke.py`, manifest evidence tests, and the Python gate
  smoke suite pass; OCI inventory verification remains deliberately
  `release_evidence=false` without signing material.

The full live absent-Broker Heist remains unproven: no real offer/claim with
invocation context and completion has been promoted, and the full live UI,
replay, restart/kill recovery, cross-backend, native distribution, and
external release acceptance rows remain open. All 15 active project issues
therefore remain In Progress; no release flag, bearer, artifact, or Linear Done
state was fabricated. James the 2nd's fail-closed blocker report is
`docs/agents/imo-53-57-live-wave9.md`: the daemon still lacks a public
transition-producing operation (or pre-seeded eligible Invocation fixture) that
could create a real Heist Activation from a fresh Room.

## Parent continuation addendum (2026-08-21, Wave 8 action-reply correlation)

Turing the 2nd fixed a concrete IMO-55 protocol boundary in
`crates/worldstream-server/src/lib.rs`: dispatch replies and errors now echo an
envelope's explicit `request_id`, while messages without one retain the
`message_id` fallback. The regression covers both correlation paths; no
compatibility manifest was modified. The bounded lane report is
`docs/agents/imo-55-action-reply-wave8.md`.

Parent review passed the focused server suite (38 tests), strict server
Clippy, Rust formatting, diff checks, and the full Python SDK suite (39 tests)
with Ruff checks. After rebuilding `worldstreamd`, the real-process runner
  passed HTTP room creation, member-capability issuance, WebSocket attach/sync/
  ack, restart/idempotency, and `Room.act("increment")`; the action reply was
  correlated and advanced the Counter room head from sequence 0 to 1 without
  emitting secrets. The historical Wave 8 runner was `partial` because the
  scheduler was then unconfigured; Wave 9 starts it, but still does not promote
  the full Agent Heist story or release readiness.
