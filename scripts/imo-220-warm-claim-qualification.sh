#!/usr/bin/env bash
set -euo pipefail

# Produces local, redacted evidence for IMO-220's SQLite warm Activation path.
#
# The history fixture creates each source through production Core/storage APIs.
# The server test then appends one authorized Agent Membership, measures a
# cache-miss claim plus bounded real current-read, accepted Action, stale
# Action, and fresh claim/release paths, and deliberately corrupts an in-head
# historical record after cache installation. The test source is disposable:
# only the combined JSON evidence survives.

readonly SCHEMA="worldstream/imo-220-warm-claim-evidence/v2"
readonly DEFAULT_SAMPLES=1000
readonly DEFAULT_TIERS=(1000 10000 100000)

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

cargo_bin="${WORLDSTREAM_IMO220_CARGO:-cargo}"
python_bin="${WORLDSTREAM_IMO220_PYTHON:-/usr/bin/python3}"
history_fixture_bin="${WORLDSTREAM_HISTORY_FIXTURE_BIN:-}"
warm_test_bin="${WORLDSTREAM_WARM_TEST_BIN:-}"
evidence_file=""
samples="$DEFAULT_SAMPLES"
temp_root=""
tiers=()
history_fixture_mode="cargo-release"
history_fixture_sha256=""
warm_test_mode="cargo-release"
warm_test_sha256=""

usage() {
  cat <<'USAGE'
Usage: scripts/imo-220-warm-claim-qualification.sh --evidence PATH [--samples N] [--tier N]...

By default, creates the ticket's 1k, 10k, and 100k canonical SQLite histories
in a private temporary location. Repeat --tier to select one or more supported
tiers: 1000, 10000, 100000, or 1000000. It retains a redacted JSON summary at
PATH and deletes the sources, logs, and database sidecars on exit. PATH must
not already exist.

Set WORLDSTREAM_HISTORY_FIXTURE_BIN and WORLDSTREAM_WARM_TEST_BIN to reusable
release binaries to avoid Cargo builds. The evidence records only each supplied
binary's SHA-256, never its path. The warm test binary is invoked with its
exact filter and --ignored/--nocapture test-harness arguments.
USAGE
}

file_sha256() {
  if command -v shasum >/dev/null; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    printf '%s\n' 'IMO-220 requires shasum or sha256sum for prebuilt artifact identity' >&2
    return 127
  fi
}

cleanup() {
  if [[ -n "$temp_root" && -d "$temp_root" ]]; then
    "$python_bin" - "$temp_root" <<'PY'
import shutil
import sys
shutil.rmtree(sys.argv[1])
PY
  fi
}
trap cleanup EXIT

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --evidence)
      [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }
      evidence_file="$2"
      shift 2
      ;;
    --samples)
      [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }
      samples="$2"
      shift 2
      ;;
    --tier)
      [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }
      tiers+=("$2")
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

[[ -n "$evidence_file" ]] || { usage >&2; exit 2; }
[[ "$samples" =~ ^[1-9][0-9]*$ ]] || { printf '%s\n' 'IMO-220 samples must be a positive integer' >&2; exit 2; }
[[ ! -e "$evidence_file" ]] || { printf '%s\n' 'IMO-220 evidence destination already exists' >&2; exit 2; }
command -v "$python_bin" >/dev/null || { printf '%s\n' 'IMO-220 Python executable is unavailable' >&2; exit 127; }
if [[ -z "$history_fixture_bin" || -z "$warm_test_bin" ]]; then
  command -v "$cargo_bin" >/dev/null || { printf '%s\n' 'IMO-220 cargo executable is unavailable' >&2; exit 127; }
fi
if [[ -n "$history_fixture_bin" ]]; then
  [[ -f "$history_fixture_bin" && -x "$history_fixture_bin" && ! -L "$history_fixture_bin" ]] || {
    printf '%s\n' 'IMO-220 history fixture binary must be an executable regular file' >&2
    exit 2
  }
  history_fixture_mode="prebuilt"
  history_fixture_sha256="$(file_sha256 "$history_fixture_bin")"
fi
if [[ -n "$warm_test_bin" ]]; then
  [[ -f "$warm_test_bin" && -x "$warm_test_bin" && ! -L "$warm_test_bin" ]] || {
    printf '%s\n' 'IMO-220 warm test binary must be an executable regular file' >&2
    exit 2
  }
  warm_test_mode="prebuilt"
  warm_test_sha256="$(file_sha256 "$warm_test_bin")"
fi

if [[ "${#tiers[@]}" -eq 0 ]]; then
  tiers=("${DEFAULT_TIERS[@]}")
fi
for tier in "${tiers[@]}"; do
  case "$tier" in
    1000|10000|100000|1000000) ;;
    *)
      printf '%s\n' "IMO-220 tier must be one of 1000, 10000, 100000, or 1000000: ${tier}" >&2
      exit 2
      ;;
  esac
done
if [[ "$(printf '%s\n' "${tiers[@]}" | sort -n | uniq -d)" != "" ]]; then
  printf '%s\n' 'IMO-220 tiers must be unique' >&2
  exit 2
fi

umask 077
temp_root="$("$python_bin" - <<'PY'
import tempfile
print(tempfile.mkdtemp(prefix='worldstream-imo220-warm-', dir='/tmp'))
PY
)"

for tier in "${tiers[@]}"; do
  source_database="$temp_root/history-${tier}.sqlite"
  fixture_report="$temp_root/history-${tier}.json"
  fixture_log="$temp_root/history-${tier}.fixture.log"
  warm_report="$temp_root/history-${tier}.warm.json"
  warm_log="$temp_root/history-${tier}.warm.log"

  if [[ "$history_fixture_mode" == "prebuilt" ]]; then
    fixture_command=("$history_fixture_bin")
  else
    fixture_command=("$cargo_bin" run --locked --release -p worldstream-sqlite --example history_qualification_fixture --)
  fi
  if ! "${fixture_command[@]}" \
    --database "$source_database" \
    --transition-count "$tier" \
    --stream-metadata \
    --output "$fixture_report" \
    >"$fixture_log" 2>&1; then
    printf 'IMO-220 fixture failed for %s transitions; tail follows:\n' "$tier" >&2
    tail -n 160 "$fixture_log" >&2
    exit 1
  fi

  if [[ "$warm_test_mode" == "prebuilt" ]]; then
    warm_command=("$warm_test_bin" source_backed_warm_activation_claim_qualification --ignored --nocapture)
  else
    warm_command=("$cargo_bin" test --locked --release -p worldstream-server --lib source_backed_warm_activation_claim_qualification -- --ignored --nocapture)
  fi
  if ! env \
    WORLDSTREAM_WARM_CLAIM_MUTABLE_SOURCE=1 \
    WORLDSTREAM_WARM_CLAIM_SOURCE_DB="$source_database" \
    WORLDSTREAM_WARM_CLAIM_EXPECTED_HISTORY="$tier" \
    WORLDSTREAM_WARM_CLAIM_SAMPLES="$samples" \
    WORLDSTREAM_WARM_CLAIM_EVIDENCE_FILE="$warm_report" \
    "${warm_command[@]}" \
      >"$warm_log" 2>&1; then
    printf 'IMO-220 warm-claim test failed for %s transitions; tail follows:\n' "$tier" >&2
    tail -n 160 "$warm_log" >&2
    exit 1
  fi

  grep -Fq 'IMO220_SQLITE_WARM_CLAIM_EVIDENCE=' "$warm_log" || {
    printf '%s\n' "IMO-220 warm evidence marker missing for ${tier}" >&2
    tail -n 160 "$warm_log" >&2
    exit 1
  }
done

"$python_bin" - "$evidence_file" "$temp_root" "$samples" "$history_fixture_mode" "$history_fixture_sha256" "$warm_test_mode" "$warm_test_sha256" "${tiers[@]}" <<'PY'
import json
import os
import sys

output, root, samples, fixture_mode, fixture_sha256, warm_mode, warm_sha256, *tier_strings = sys.argv[1:]
tiers = [int(value) for value in tier_strings]
rows = []
for tier in tiers:
    with open(os.path.join(root, f"history-{tier}.json"), encoding="utf-8") as handle:
        fixture = json.load(handle)
    with open(os.path.join(root, f"history-{tier}.warm.json"), encoding="utf-8") as handle:
        warm = json.load(handle)
    if fixture.get("schema") != "worldstream/room-history-qualification/sqlite-v1":
        raise SystemExit(f"unexpected fixture schema for {tier}")
    if fixture.get("pass") is not True:
        raise SystemExit(f"fixture V2 checkpoint qualification failed for {tier}")
    if fixture.get("requested_transition_count") != tier:
        raise SystemExit(f"fixture requested count mismatch for {tier}")
    counters = fixture.get("counters", {})
    if any(not isinstance(counters.get(field), int) or counters[field] < 0 for field in (
        "models", "invocations", "attempts", "transitions", "frames",
    )):
        raise SystemExit(f"fixture canonical counters missing for {tier}")
    if any(counters.get(field) != tier for field in ("models", "invocations", "attempts", "transitions")):
        raise SystemExit(f"fixture canonical transition count mismatch for {tier}")
    history = fixture.get("history", {})
    if history.get("head_room_seq") != tier:
        raise SystemExit(f"fixture canonical head mismatch for {tier}")
    if any(not isinstance(history.get(field), int) or history[field] < 0 for field in (
        "warm_path_reads", "reducer_callbacks", "recovery_ms", "context_bytes",
    )):
        raise SystemExit(f"fixture recovery metrics missing for {tier}")
    storage = fixture.get("storage", {})
    if any(not isinstance(storage.get(field), int) or storage[field] < 0 for field in (
        "db_bytes", "wal_bytes", "shm_bytes", "transitions", "frames",
    )):
        raise SystemExit(f"fixture storage metrics missing for {tier}")
    if storage.get("rss_bytes") is not None and (
        not isinstance(storage["rss_bytes"], int) or storage["rss_bytes"] < 0
    ):
        raise SystemExit(f"fixture storage RSS metric malformed for {tier}")
    fixture_checkpoint = fixture.get("checkpoint", {})
    if fixture_checkpoint.get("recovery_execution_path") != "checkpoint_v2":
        raise SystemExit(f"fixture did not select a V2 checkpoint for {tier}")
    if not isinstance(fixture_checkpoint.get("checkpoint_room_seq"), int):
        raise SystemExit(f"fixture V2 checkpoint sequence missing for {tier}")
    if any(not isinstance(fixture_checkpoint.get(field), int) or fixture_checkpoint[field] < 0 for field in (
        "checkpoint_boundary_transition_records_read_by_adapter",
        "prefix_transition_range_reads",
        "prefix_transition_records_delivered_to_core",
        "prefix_transitions_skipped",
        "tail_transition_records_delivered_to_core",
        "transition_records_read_by_adapter_total",
        "witness_bytes",
    )):
        raise SystemExit(f"fixture checkpoint read/tail metrics missing for {tier}")
    snapshots = fixture.get("snapshots", {})
    if any(not isinstance(snapshots.get(field), int) or snapshots[field] < 1 for field in (
        "preparation_count", "write_count", "observed_snapshot_count",
    )):
        raise SystemExit(f"fixture snapshot cadence evidence missing for {tier}")
    if snapshots["preparation_count"] != snapshots["write_count"]:
        raise SystemExit(f"fixture snapshot preparation/write mismatch for {tier}")
    if warm.get("schema") != "worldstream/imo-220-warm-claim-qualification/sqlite-v2":
        raise SystemExit(f"unexpected warm schema for {tier}")
    if warm.get("source", {}).get("fixture_transition_rows") != tier:
        raise SystemExit(f"fixture count mismatch for {tier}")
    checkpoint_contract = warm.get("source", {}).get("checkpoint_v2_contract", {})
    if checkpoint_contract.get("schema") != "worldstream/checkpoint-operational-witness/v2":
        raise SystemExit(f"V2 checkpoint witness schema mismatch for {tier}")
    if checkpoint_contract.get("recovered_current_head_room_seq") != tier:
        raise SystemExit(f"V2 recovered head mismatch for {tier}")
    if checkpoint_contract.get("checkpoint_room_seq") != checkpoint_contract.get("checkpoint_head_room_seq"):
        raise SystemExit(f"V2 checkpoint boundary mismatch for {tier}")
    if checkpoint_contract.get("checkpoint_room_seq", tier + 1) > tier:
        raise SystemExit(f"V2 checkpoint lies beyond current head for {tier}")
    if checkpoint_contract.get("witness_hash_exact") is not True or checkpoint_contract.get("witness_snapshot_head_exact") is not True or checkpoint_contract.get("durable_operational_root_inventory_exact") is not True or checkpoint_contract.get("durable_operational_roots_exact") is not True:
        raise SystemExit(f"V2 witness/root contract failed for {tier}")
    if checkpoint_contract.get("durable_operational_roots_verified_by") != "guarded_checkpoint_v2_recovery":
        raise SystemExit(f"V2 final-root guard proof missing for {tier}")
    if not isinstance(checkpoint_contract.get("durable_operational_root_count"), int) or checkpoint_contract["durable_operational_root_count"] < 1:
        raise SystemExit(f"V2 operational root inventory missing for {tier}")
    if warm.get("source", {}).get("canonical_transition_rows_after_agent_join") != tier + 1:
        raise SystemExit(f"Agent Join canonical count mismatch for {tier}")
    if warm.get("source", {}).get("canonical_transition_rows_after_warm_actions") != tier + 3:
        raise SystemExit(f"warm Action canonical count mismatch for {tier}")
    actions = warm.get("warm_actions", {})
    if actions.get("accepted_count") != 2:
        raise SystemExit(f"accepted warm Action count mismatch for {tier}")
    if actions.get("canonical_transition_rows_before") != tier + 1 or actions.get("canonical_transition_rows_after") != tier + 3:
        raise SystemExit(f"warm Action canonical row evidence mismatch for {tier}")
    if actions.get("reducer_callbacks_after") != actions.get("reducer_callbacks_before", -2) + 2:
        raise SystemExit(f"warm Action reducer evidence mismatch for {tier}")
    reads = warm.get("warm_current_reads", {})
    if reads.get("read_count") != int(samples):
        raise SystemExit(f"warm current-read sample count mismatch for {tier}")
    if reads.get("reducer_callbacks_before") != reads.get("reducer_callbacks_after"):
        raise SystemExit(f"warm current read invoked a reducer for {tier}")
    if warm.get("warm_claims", {}).get("claim_count") != int(samples):
        raise SystemExit(f"warm sample count mismatch for {tier}")
    if warm.get("warm_claims", {}).get("reducer_callbacks_before") != warm.get("warm_claims", {}).get("reducer_callbacks_after"):
        raise SystemExit(f"warm claim invoked a reducer for {tier}")
    if warm.get("corruption_proof", {}).get("recovery_forbidden_after_corruption") is not True:
        raise SystemExit(f"missing recovery fence proof for {tier}")
    if warm.get("corruption_proof", {}).get("warm_actions_accepted_after_corruption") != 2:
        raise SystemExit(f"warm Action corruption proof failed for {tier}")
    if warm.get("corruption_proof", {}).get("warm_current_reads_served_after_corruption") != int(samples):
        raise SystemExit(f"warm current-read corruption proof failed for {tier}")
    if warm.get("corruption_proof", {}).get("canonical_transition_rows_unchanged_after_warm_actions") is not True:
        raise SystemExit(f"canonical row proof failed for {tier}")
    stale = warm.get("stale_actions", {})
    if stale.get("submitted_count") != 4 or stale.get("stale_rejection_count") != 4 or stale.get("stale_rate") != 1.0:
        raise SystemExit(f"stale Action rate evidence mismatch for {tier}")
    queue = warm.get("activation_queue", {})
    if queue.get("operator_waiting") != 1 or queue.get("operator_leased") != 0 or queue.get("durable_pending_rows") != 1:
        raise SystemExit(f"Activation queue evidence mismatch for {tier}")
    if not isinstance(queue.get("oldest_pending_age_ms"), int) or queue["oldest_pending_age_ms"] < 0:
        raise SystemExit(f"Activation queue age missing for {tier}")
    if warm.get("activation_final", {}).get("state") != "pending":
        raise SystemExit(f"Activation did not return to pending for {tier}")
    if warm.get("fence", {}).get("head_room_seq") != tier + 3:
        raise SystemExit(f"warm serving fence head mismatch for {tier}")
    if warm.get("fence", {}).get("head_stable_after_warm_actions") is not True:
        raise SystemExit(f"warm serving fence changed after Actions for {tier}")
    rows.append({
        "history_tier": tier,
        "fixture": fixture,
        "warm_activation": warm,
    })
payload = {
    "schema": "worldstream/imo-220-warm-claim-evidence/v2",
    "backend": "sqlite",
    "samples_per_tier": int(samples),
    "qualification": {
        "pass": True,
        "checkpoint_contract": "v2 witness hash and selected snapshot head verified against durable V2 operational roots",
    },
    "execution": {
        "history_fixture": {"mode": fixture_mode, "sha256": fixture_sha256 or None},
        "warm_test": {"mode": warm_mode, "sha256": warm_sha256 or None},
    },
    "tiers": rows,
    "redaction": {
        "database_paths": "omitted",
        "bearers": "omitted",
        "connection_strings": "omitted",
    },
}
serialized = json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"
flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
fd = os.open(output, flags, 0o600)
with os.fdopen(fd, "w", encoding="utf-8") as handle:
    handle.write(serialized)
PY

tier_csv="$(IFS=,; printf '%s' "${tiers[*]}")"
printf 'IMO220_WARM_CLAIM_EVIDENCE=PASS schema=%s tiers=%s samples=%s\n' "$SCHEMA" "$tier_csv" "$samples"
