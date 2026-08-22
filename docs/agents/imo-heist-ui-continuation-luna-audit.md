# IMO-53–IMO-57 Heist/UI continuation acceptance audit

Date: 2026-08-21
Auditor: read-only acceptance audit
Checkout: `/Users/vinothshanmugam/code/agent-streamer`

## Result

No issue is fully accepted by this audit. The reducer, privacy policy, protocol, SDK, and UI state-machine seams have substantial direct test evidence. The required end-to-end absent-Broker Heist story, process-restart durability proof, retained executable/privacy corpus, exact live hash set, and browser-backed UI proof are not established.

The current checkout was dirty before this audit. No source, manifest, gate, Linear status, or existing file was changed. The only file created is this audit.

Status meanings:

- **Proven** — the checkbox is directly covered at its stated scope by source-plus-test or current live evidence.
- **Incomplete** — a material part is covered, but the checkbox asks for more than the evidence demonstrates.
- **Contradicted** — repository metadata or an executable check explicitly conflicts with the checkbox.
- **Blocked** — the required live/process/retained evidence is unavailable or the current executable path fails before the required assertion.

“Offline story” means the independent Python reducer in `examples/heist/story.py`; it is not direct evidence of the Rust daemon or first-party live UI. “Narrative” means a prior report without a retained raw run artifact.

## Hash and artifact finding

The current offline fixture produces these SHA-256 values:

| Value | SHA-256 |
|---|---|
| transcript | `c31a01685be5d16cebba477e16c51d0a923b8ca30202e7fd0724d10dc5f505e8` |
| Genesis | `668b98ad3b24c751bfcfd7f22307f7c6ba90188def91f00be4594475de2c1cf9` |
| final Core | `53a55ca514d3e1e950be3a4d333854a519007f5f9aebd11d39358c0ae774328a` |
| final Activity | `a55c41b8ec66d6d7a8ecfc3f48188ec1b20f1346936d950d81ed54fbc7aa517b` |
| final authoritative/aggregate | `a63fa15f679ab486627052ed482c4e8bdf2b9a83a5579078c9df757c7c029831` |
| final lineage | `6ae8ad69d00de0c6489a427818f03f4bcaed75919d207470b4420593369b0cae` |

The 15 offline transition hashes generated in the audit run were:

```text
01 80c42d0d6100d9a8eca86d6e65cd3efb85f968a4f6105d19f1549537405d1524
02 1ea205a72b1d2892cd4e8a493d477bd7595d897649a23d5271786082720337e4
03 6e2e05eb8d94eb0007afc10366fbf786d96308b359ac76f6d0a28e651eeb37a9
04 509331be77c8090c4970347101115e43208b68500cddf6a3d2471f9905584a0c
05 ab8449b4a7fc543fda99c3d2460ce18a45ec66dedd984807490e2b5c78a33178
06 fe78b2f6f19aa14e1ddce0aef53498faf8bd46fccdb4e02df329473be21545dc
07 8c1fe4b1cfdbc93391cf16383a47361be9fa0f82e8e7381330bf48a03cc387e4
08 214719a11165df09e3899dd2525050ad18d983286836c0eaf9c2d1dd817b6574
09 249e8c91d0662a2aef9a12b3a1de710d4d77d0ca845322c42ffde7d66a7e3796
10 587d31e8a9b7cc929d1e5609c106d11bbf3739f97a768ef00bd922c3970255d0
11 91572d9d2d81949ad7e053146c1642696328261c81bab5e344db44cfd16c7fc8
12 d6b91b0ef8b123a0a2d1e87f6b672058f46d5196c694a50afd62bcab13ebe7eb
13 5baf49c758d12b06daa3859bc0ac33ea13625377cea971a19344841aea9657fd
14 d9243a754b45d53fe7dd681dd42b29d4b83c19c0378163770cc23217de46dbd4
15 6ae8ad69d00de0c6489a427818f03f4bcaed75919d207470b4420593369b0cae
```

These are ephemeral offline SHA-256 evidence, not retained daemon output and not the Rust `blake3-canonical-json-v1` pack/hash contract. The current Rust Heist pack identity in `examples/heist/wave10_live/run_absent_broker_live.py:37` is `blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe`; the registry’s fixed transcript test uses `blake3:33949bd5b664af05341193f8721ed9abe801de28fdb9b8a5b55176bd5c1f9ec8` (`crates/worldstream-core/src/agent_heist_registry.rs:20-24`). These must not be conflated.

No retained raw JSON/log/NDJSON daemon result was found. The retained material is narrative Markdown and the bounded fixture report `docs/agents/imo-sqlite-durability-continuation-soak.json`, which explicitly says `evidence_class: bounded_fixture_only`, `process_kill_claim: false`, `process_level: false`, and `kill_points.status: not_exposed`.

The current live Heist harness does not emit exact Core/Activity/aggregate/Transition hashes: `examples/heist/wave10_live/run_absent_broker_live.py:984-1027` emits phase/outcome/commitment/replay-verification fields only. `replay_verified: true` is not equality evidence for the four requested hash families.

## Checkbox matrix

### IMO-53 — Execute the six-phase Agent Heist reducer

| # | Checkbox | Status | Evidence and gap |
|---:|---|---|---|
| 1 | Room seed selects one versioned fixture and exactly three immutable Genesis seats; suspension/departure leaves an explicit missing seat without changing denominator or transferring knowledge. | **Incomplete** | Direct initialization and fixed seats are in `crates/worldstream-core/src/agent_heist.rs:273-326`; outcome/missing-seat handling is in `:1030-1108`. `agent_heist_privacy_tests.rs:950-973` tests a suspended seat, but does not exercise an actual Departed state or directly prove the full denominator/knowledge invariant. |
| 2 | Briefing/Negotiation actions are bounded inspect, publish, exchange, plan, endorsement, and challenge with stable declared rejections. | **Incomplete** | Handlers are implemented at `agent_heist.rs:380-825` and the descriptor/action rejection declarations at `:2074-2105`. The direct Rust tests do not cover every listed action/rejection; the Python story is a separate reducer, and the current live daemon run stopped before an offer/action. |
| 3 | Commitment enforces `open_at <= admitted_at < deadline`, reminder, generation-fenced early third-commit closure, strictly-later resolve timer, and deadline closure with missing seats. | **Proven** | Bound is asserted in `agent_heist.rs:731-760`; early third-commit cancellation and resolve scheduling are tested in `agent_heist_privacy_tests.rs:444-552`; six-phase/generation/duplicate-timer behavior is tested in `:554-717`; absent-Broker deadline/result path is tested in `:719-751`. This is reducer/timer evidence, not a live scheduler run. |
| 4 | Golden matrices cover zero, one, two matching, two split, 3-0, 2-1, and three split; 5/5 succeeds, 3-4/5 is partial, and 0-2/5/no majority fails. | **Incomplete** | Direct matrix coverage exists at `agent_heist_privacy_tests.rs:381-442` and `agent_heist_registry.rs:198-262`: zero/one/two-match, two-split, 3-0, 2-1, and three-split. No score-3 row is covered; only score 4 is shown for the stated “3-4/5 partial” range. |
| 5 | Result/Complete transitions, timer cancellation, duplicate/stale delivery, crash-after-commit, and Genesis-fold Replay reproduce exact state/events/timers/hashes. | **Blocked** | In-process transition/replay/timer tests exist (`agent_heist_privacy_tests.rs:554-717`; SQLite suite), but no retained process-kill artifact proves crash-after-commit or exact daemon state/event/timer/hash reproduction. The bounded soak report explicitly disclaims process-kill evidence, and the current live run blocked at the first timer. |
| 6 | No Heist-specific branch or new kernel primitive is introduced. | **Proven** | The pack implements the ordinary `ActivityPackV1` path (`agent_heist.rs:241-270`) and is composed through the normal registry (`crates/worldstream-core/src/registry.rs:21-25`). This is source inspection; it does not imply live acceptance. |

### IMO-54 — Prove Heist privacy, Attention, and retained executability

| # | Checkbox | Status | Evidence and gap |
|---:|---|---|---|
| 1 | Live views expose only frozen public/participant/operator fields; operators never receive raw Activity State, private clues/offers, or sealed commitments. | **Proven** | Core public/operator/historical assertions are direct in `agent_heist_privacy_tests.rs:873-891`; projection allowlists/private fields are in `agent_heist.rs:1255-1375`. UI allowlist/redaction is tested in `web/console/src/liveSession.test.ts:141-181`. Evidence is in-process/fixture-backed, not a completed live Heist frame capture. |
| 2 | Commitments stay sealed through Result; only authorized post-Complete final reveal is allowed; historical Replay does not inherit later authority. | **Proven** | Sealed commitment event assertions: `agent_heist_privacy_tests.rs:240-256`; final-reveal gate and historical redaction: `:894-948`; SQLite authorized historical replay/final-reveal fencing tests pass. |
| 3 | Paired hidden-state fixtures are indistinguishable through current projection, frame, catch-up, reset, error, log/UI payload, and Replay. | **Incomplete** | `examples/heist/test_story.py:176-207` directly compares the offline Python surfaces, and core/UI tests cover individual projection paths. There is no real server Heist fixture pairing all listed frame/catch-up/reset/error/log/Replay paths; the current live run never reached an offer or frame. |
| 4 | Five Attention reason codes and precedence are enforced, at most one reason exists per target/Transition, and policy decisions create operational intents while Replay verifies signals only. | **Proven** | Five codes are declared at `agent_heist.rs:2107-2115`; precedence and one-signal assertions are direct at `agent_heist_privacy_tests.rs:258-284`. The operational/replay boundary is documented and implemented in `crates/worldstream-core/src/activation.rs:1-6,72-99`, with activation/recovery tests passing. |
| 5 | Old Heist digests fully load/project/advance/replay when not selectable for new Rooms; cross-platform golden corpus bytes/hashes are fixed. | **Contradicted** | The registry marks the current retained pack runnable and new-selectable (`agent_heist_registry.rs:26-54`), but `compatibility.toml:402-417` has the Heist revision/descriptor/executor/schema/codec/golden-corpus digest fields empty, `status = unresolved`, and `release_ready = false`. No old non-selectable Heist corpus and fixed cross-platform byte/hash artifact was found. |
| 6 | SQLite crash/recovery with all snapshots removed reproduces every phase, Outcome, explanation, view, and Attention Signal. | **Blocked** | The 90-test SQLite suite has generic recovery, replay, activation, and snapshot cases, but no full Heist all-snapshots-removed run. The retained soak explicitly says fixture-only and no process kill; the current live Heist run is timer-blocked. |

### IMO-55 — Complete a Counter Room through protocol and Python SDK

| # | Checkbox | Status | Evidence and gap |
|---:|---|---|---|
| 1 | Versioned envelopes have size/rate bounds, strict schemas, capability scopes, exact Head handling, stable/transient outcomes, and secret-safe errors. | **Proven** | Protocol/server/SDK tests pass: `cargo test --locked -p worldstream-protocol` (12), `worldstream-server` (48), and SDK tests (33). Direct checks include `web/console/src/transport.test.ts:43-140`, server auth/action tests, and no-secret live output. |
| 2 | Python client atomically installs retained frames or Projection Reset, ACKs the Session token, streams Live, reconnects, and handles pruning/slow-consumer closure. | **Proven** | `sdk/python/tests/test_room_client.py:253-382,652-716` directly covers sync barriers, reset installation, reconnect/ACK cursor rules, retained sync, and slow-consumer closure. Current Counter live evidence also reached capability issuance, attach, sync/observation ACK, action, restart, and reattach. |
| 3 | Lost Action reply, duplicate identity, changed-payload conflict, hidden Head advance, stable stale result, resync, and new Action identity work without internal APIs. | **Proven** | SDK direct tests: `test_room_client.py:404-460,719-738`; server Counter action/idempotency/stale tests: `crates/worldstream-server/src/sqlite_backend.rs:2450-2608`. The live Counter harness also exercised an externally issued action and correlated reply. |
| 4 | Two Memberships expose only authorized Counter views; Replay/current respect present and historical authority. | **Incomplete** | Generic two-member/private-frame and authorized historical replay tests pass in the SQLite suite; SDK cross-membership rejection is direct. The retained/current live run has only one membership and does not prove a two-member Counter daemon run with current and historical Replay authorization. |
| 5 | A scripted scenario restarts SQLite and reaches the same projection/hashes. | **Incomplete** | `examples/heist/wave6-live/run_live_story.py` proves a real Counter daemon restart, create idempotency, same room/member IDs, attach, and ACK, but reports `status: partial` and does not compare post-action projections or exact hashes after restart. |

### IMO-56 — Operate Agent Heist through first-party UI

| # | Checkbox | Status | Evidence and gap |
|---:|---|---|---|
| 1 | Public UI shows phase/deadline/role presence/public clues/plans/endorsements/challenges/commit count/aggregate result and no private data. | **Proven** | Public projection/allowlist checks are direct in `web/console/src/liveSession.test.ts:141-181` and `web/console/src/App.test.tsx:18-30`; matching core privacy tests are in `agent_heist_privacy_tests.rs`. This proves the rendered projection/fixture boundary, not a live browser session. |
| 2 | Participant can inspect/disclose/exchange/propose/challenge/commit/ack only through current Action Offers and exact-Head submissions. | **Incomplete** | Exact offer/head enforcement is tested in `liveSession.test.ts:48-57,183-210` and `transport.test.ts:124-140`. `App.test.tsx:81-186` explicitly exercises fixture-gated controls with no live submission; no browser-backed live Heist Action path was demonstrated. |
| 3 | Operator Membership/session/runner/timer/frame/Activation/integrity diagnostics never expose raw Core/Activity/sealed/private values. | **Proven** | Operator diagnostics are bounded in `App.test.tsx:32-42`, with core operator privacy assertions in `agent_heist_privacy_tests.rs:873-891`. |
| 4 | Attach, retained catch-up, reset install, stale-result resync/new-ID, Loading/CatchingUp denial, Faulted, and Quarantined are visible and tested. | **Proven** | UI state and error assertions are in `App.test.tsx:44-56,81-146,164-186`; transport/live-session recovery paths are in `liveSession.test.ts:59-139,222-235` and `transport.test.ts:184-263`. This is state-machine/test evidence, not browser-backed live Heist evidence. |
| 5 | Replay is read-only, historical authorization is honored, and final reveal is unavailable before Complete. | **Proven** | UI Replay/final gate assertions: `App.test.tsx:58-79`; core final/historical tests: `agent_heist_privacy_tests.rs:894-948`; SDK read-only Replay checks: `sdk/python/tests/test_room_client.py:741-783`. |
| 6 | Browser/DOM privacy tests and production build pass, with no provider credentials. | **Incomplete** | `pnpm ui:build`, lint, and UI tests pass; `web/console/src/privacyDom.test.tsx` and App tests inspect rendered markup and no credentials are emitted. However `web/console/package.json:12` runs Vitest with `--environment node`, and `privacyDom.test.tsx` uses `renderToStaticMarkup`; no real browser/DOM automation proof exists. |

### IMO-57 — Run the full absent-Broker Agent Heist story

| # | Checkbox | Status | Evidence and gap |
|---:|---|---|---|
| 1 | Deterministic Navigator/Insider/Broker strategies drive all six phases to the expected Outcome. | **Blocked** | The offline Python story gives a deterministic six-phase path and success score 5 with missing Broker, but is a separate reducer. Current `run_absent_broker_live.py --spawn-daemon` contacted a real daemon and stopped at the first timer with `status: blocked`, `reason_code: timer_not_due_or_not_publicly_addressable`, no offer/claim/final Outcome. No retained raw completed daemon artifact was found. |
| 2 | Broker’s first Invocation ends; later Attention produces one intent; a fresh Invocation has exact context; a separately authorized participant client submits the Action. | **Blocked** | Offline `ActivationLedger` tests cover separated authority/context, but no live offer/claim was reached. The current run has `fabricated_offer: false`, no private contexts, and no participant Action. |
| 3 | Duplicate offer, lost claim reply, lease expiry/reclaim, stale generation, and idempotent completion are safe and do not advance Cursor incorrectly. | **Incomplete** | SDK fake-runner tests and server fault-boundary unit tests cover retry/lease/idempotency semantics (`sdk/python/tests/test_runner_client.py:145-262`; `worldstream-server` kill/lost-claim tests). Prior Markdown reports claim disposable fault runs, but no raw artifacts are retained; no full live Heist Cursor trace is available. |
| 4 | Kill after Room COMMIT before reply/frame publication loses no result; restart drains overdue timers before new Actions and resumes pending delivery/Activation. | **Blocked** | The retained SQLite soak explicitly has no process-kill evidence. Narrative reports claim fault runs but provide no raw receipt/frame/hash artifact, and the current live Heist run is blocked at timer readiness. No direct full restart/drain/resume story was reproduced. |
| 5 | First-party UI has the same privacy story, and final Replay has identical Core/Activity/aggregate/Transition hashes. | **Contradicted** | UI tests only assert an offline transcript digest/replay count (`web/console/src/App.test.tsx:7-16`). The current live harness emits only `replay_verified`, not the four exact hash families (`run_absent_broker_live.py:984-1027`). Offline values are SHA-256 and explicitly not Rust BLAKE3 (`examples/heist/README.md`); the current live run is blocked before final Replay. |
| 6 | A fresh-checkout script runs the SQLite story without paid models or private services. | **Incomplete** | `examples/heist/test_fresh_checkout.py:13-39` copies the Python story files into a temp checkout and runs the offline self-test; it does not start SQLite or a daemon. The live harness uses a built daemon and temporary local data, but the current full story is blocked and no fresh-checkout SQLite run artifact is retained. |

## Checks executed

All commands below were read-only. The repository’s pyenv default requested unavailable Python 3.14.7, so the existing `sdk/python/.venv/bin/python` was used.

| Check | Result |
|---|---|
| `PYTHONPATH=examples/heist sdk/python/.venv/bin/python -m unittest discover -s examples/heist -p 'test_*.py' -q` | 19 passed |
| `PYTHONPATH=sdk/python/src:examples/heist/wave10_live sdk/python/.venv/bin/python -m pytest -q examples/heist/wave10_live/test_absent_broker_live.py` | 5 passed |
| `sdk/python/.venv/bin/python -m pytest -q sdk/python/tests` | 33 passed |
| Ruff check/format check for SDK and Heist examples | Passed; 19 files formatted |
| `pnpm ui:lint` | Passed; Node engine warning only |
| `pnpm ui:test -- --run` | 4 files, 30 passed; Node/Vitest node-environment limitation noted above |
| `pnpm ui:build` | Passed |
| `cargo test --locked -p worldstream-core --lib agent_heist_privacy_tests -- --nocapture` | 15 passed |
| `cargo test --locked -p worldstream-core --lib agent_heist_registry -- --nocapture` | 2 passed |
| `cargo test --locked -p worldstream-server --lib` | 48 passed |
| `cargo test --locked -p worldstream-sqlite --lib` | 90 passed |
| `cargo test --locked --workspace` | Passed; all displayed workspace targets passed |
| `PYTHONPATH=sdk/python/src sdk/python/.venv/bin/python examples/heist/wave6-live/run_live_story.py` | Real disposable Counter daemon: `status: partial`; health/ready 200, create idempotency, capability, attach, sync/observation ACK, action, restart/reattach proved |
| `PYTHONPATH=sdk/python/src sdk/python/.venv/bin/python examples/heist/wave10_live/run_absent_broker_live.py --spawn-daemon --binary target/debug/worldstreamd --timer-timeout 35 --offer-timeout 2` | Real disposable daemon: exit 2, `status: blocked`, `timer_not_due_or_not_publicly_addressable`, no fabricated offer/private context/final Replay |
| `sdk/python/.venv/bin/python examples/heist/run_story.py --self-test` | Offline fixture: phase path and Replay verified; score 5/missing Broker; SHA-256 values above |

## Final acceptance conclusion

The direct evidence supports accepting substantial implementation seams: the six-phase reducer’s timer and privacy rules, Attention normalization, protocol/SDK idempotency and recovery contracts, and UI state/privacy projections. It does not support accepting the five issue descriptions as complete. The decisive missing/failed evidence is the live full Heist path, retained process-level crash/restart proof, executable retained old-digest corpus, exact live Core/Activity/aggregate/Transition hash equality, and browser-backed first-party UI privacy/action verification.
