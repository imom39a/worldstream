#!/usr/bin/env bash
set -euo pipefail

# Runs the durable fences around the SQLite warm Activation qualification.
# Only the case identifiers and pass/fail state are retained: cargo logs,
# temporary databases, and any test-generated credentials stay in a private
# temporary directory and are removed on exit.

readonly SCHEMA="worldstream/imo-220-warm-activation-safety/v1"

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

cargo_bin="${WORLDSTREAM_IMO220_CARGO:-cargo}"
python_bin="${WORLDSTREAM_IMO220_PYTHON:-/usr/bin/python3}"
evidence_file=""
temp_root=""

usage() {
  cat <<'USAGE'
Usage: scripts/imo-220-warm-safety-matrix.sh --evidence PATH

Runs exact durable-revocation, authority-fence, reset, receipt/lost-reply,
corruption, Action, and Timer qualification tests. Writes a redacted JSON
summary to PATH, which must not already exist.
USAGE
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
[[ ! -e "$evidence_file" ]] || { printf '%s\n' 'IMO-220 evidence destination already exists' >&2; exit 2; }
command -v "$cargo_bin" >/dev/null || { printf '%s\n' 'IMO-220 cargo executable is unavailable' >&2; exit 127; }
command -v "$python_bin" >/dev/null || { printf '%s\n' 'IMO-220 Python executable is unavailable' >&2; exit 127; }

umask 077
temp_root="$("$python_bin" - <<'PY'
import tempfile
print(tempfile.mkdtemp(prefix='worldstream-imo220-safety-', dir='/tmp'))
PY
)"
result_file="$temp_root/passed-cases"

readonly CASES=(
  'sqlite|registered_runner_controls_completion_release_and_durable_revocation'
  'sqlite|commit_time_expiry_and_revoke_first_fence_but_commit_first_duplicate_wins'
  'sqlite|principal_scope_and_membership_generation_drift_each_fence_the_sealed_action'
  'sqlite|recovery_install_rereads_exact_head_and_integrity_generation_before_yielding_trace'
  'sqlite|recovery_reproduces_join_reset_and_following_visibility_loss'
  'sqlite|live_suffix_fences_reset_markers_and_pruned_prefixes'
  'sqlite|activation_schema_fences_dedup_live_leases_receipts_and_context_retirement'
  'sqlite|bounded_refresh_policy_supersedes_oldest_and_retires_age_without_touching_obligations'
  'sqlite|replay_discovered_prefix_corruption_quarantines_and_fences_replay_and_mutation'
  'server|repeated_warm_activation_claims_do_not_recover_or_read_history'
  'server|warm_activation_claim_reuses_executor_after_due_timer'
)

for case_entry in "${CASES[@]}"; do
  package="${case_entry%%|*}"
  test_name="${case_entry#*|}"
  log="$temp_root/${test_name}.log"
  if [[ "$package" == "sqlite" ]]; then
    command=("$cargo_bin" test --locked -p worldstream-sqlite --lib "$test_name" -- --nocapture)
  else
    command=("$cargo_bin" test --locked -p worldstream-server --lib "$test_name" -- --nocapture)
  fi
  if ! "${command[@]}" >"$log" 2>&1; then
    printf 'IMO-220 safety case failed: %s; tail follows:\n' "$test_name" >&2
    tail -n 160 "$log" >&2
    exit 1
  fi
  if ! grep -Eq 'test result: ok\. 1 passed; 0 failed;' "$log"; then
    printf 'IMO-220 safety case did not run exactly once: %s; tail follows:\n' "$test_name" >&2
    tail -n 160 "$log" >&2
    exit 1
  fi
  printf '%s\n' "$test_name" >>"$result_file"
done

"$python_bin" - "$evidence_file" "$result_file" <<'PY'
import json
import os
import sys

output, result_file = sys.argv[1:]
with open(result_file, encoding="utf-8") as handle:
    cases = [line.strip() for line in handle if line.strip()]
payload = {
    "schema": "worldstream/imo-220-warm-activation-safety/v1",
    "backend": "sqlite",
    "case_count": len(cases),
    "cases": [{"id": case, "status": "pass"} for case in cases],
    "redaction": {
        "cargo_logs": "omitted",
        "database_paths": "omitted",
        "bearers": "omitted",
    },
}
serialized = json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"
flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
fd = os.open(output, flags, 0o600)
with os.fdopen(fd, "w", encoding="utf-8") as handle:
    handle.write(serialized)
PY

printf 'IMO220_WARM_SAFETY_EVIDENCE=PASS schema=%s cases=%s\n' "$SCHEMA" "${#CASES[@]}"
