#!/usr/bin/env bash
set -Eeuo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

output_path="${WORLDSTREAM_TELEMETRY_HTTPS_REPORT:-}"
timeout_seconds=300

usage() {
  printf '%s\n' \
    'Usage: scripts/telemetry_https_smoke.sh [--output PATH] [--timeout-seconds N]' \
    'Runs exact, bounded HTTPS/TLS transport and rejection tests. The optional report is diagnostic-only typed evidence.' >&2
}

while (($#)); do
  case "$1" in
    --output)
      (($# >= 2)) || { usage; exit 2; }
      output_path="$2"
      shift 2
      ;;
    --timeout-seconds)
      (($# >= 2)) || { usage; exit 2; }
      timeout_seconds="$2"
      shift 2
      ;;
    --help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ ! "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] || ((10#$timeout_seconds > 900)); then
  printf '%s\n' 'telemetry HTTPS smoke: timeout must be an integer from 1 through 900' >&2
  exit 2
fi

timeout_bin=""
if command -v timeout >/dev/null 2>&1; then
  timeout_bin=$(command -v timeout)
elif command -v gtimeout >/dev/null 2>&1; then
  timeout_bin=$(command -v gtimeout)
fi
command -v cargo >/dev/null 2>&1 || {
  echo "telemetry HTTPS smoke requires cargo" >&2
  exit 2
}
command -v python3 >/dev/null 2>&1 || {
  echo "telemetry HTTPS smoke requires python3" >&2
  exit 2
}

scratch=$(mktemp -d "${TMPDIR:-/tmp}/worldstream-telemetry-https.XXXXXX")
cleanup() {
  if [[ -n "${scratch:-}" && -d "$scratch" && ! -L "$scratch" ]]; then
    rm -r -- "$scratch"
  fi
}
trap cleanup EXIT

run_with_timeout() {
  if [[ -n "$timeout_bin" ]]; then
    "$timeout_bin" "${timeout_seconds}s" "$@"
    return
  fi
  python3 - "$timeout_seconds" "$@" <<'PY'
import subprocess
import sys

try:
    result = subprocess.run(
        sys.argv[2:],
        capture_output=True,
        check=False,
        timeout=int(sys.argv[1]),
    )
except subprocess.TimeoutExpired as error:
    if error.stdout:
        sys.stdout.buffer.write(error.stdout)
    if error.stderr:
        sys.stderr.buffer.write(error.stderr)
    raise SystemExit(124) from None
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
raise SystemExit(result.returncode)
PY
}

run_exact_test() {
  local check_id="$1"
  shift
  local log="$scratch/$check_id.log"
  printf 'telemetry HTTPS smoke: %s\n' "$check_id"
  if ! run_with_timeout "$@" >"$log" 2>&1; then
    sed -n '1,240p' "$log" >&2
    printf 'telemetry HTTPS smoke: exact test failed or timed out: %s\n' "$check_id" >&2
    exit 1
  fi
  sed -n '1,240p' "$log"
  python3 - "$log" "$check_id" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
counts = [
    int(value)
    for value in re.findall(r"test result: ok\. ([0-9]+) passed;", text)
]
if counts != [1]:
    raise SystemExit(
        f"telemetry HTTPS smoke: {sys.argv[2]} did not execute exactly one passing test; observed {counts}"
    )
PY
}

run_exact_test trusted_https_otlp \
  cargo test --locked -p worldstream-server --lib \
  tls_transport_uses_trusted_ca_hostname_verification_and_otlp_http -- --nocapture
run_exact_test tls_rejection \
  cargo test --locked -p worldstream-server --lib \
  tls_transport_rejects_untrusted_cert_and_hostname_mismatch -- --nocapture
run_exact_test local_otlp_http \
  cargo test --locked -p worldstream-server --lib \
  standard_http_transport_exercises_a_local_otlp_http_collector -- --nocapture
run_exact_test http_failure_bounds \
  cargo test --locked -p worldstream-server --lib \
  standard_http_transport_rejects_non_success_and_oversized_responses -- --nocapture
run_exact_test slow_response_bounds \
  cargo test --locked -p worldstream-server --lib \
  standard_http_transport_slow_response_is_bounded -- --nocapture
run_exact_test https_configuration \
  cargo test --locked -p worldstream-server --bin worldstreamd \
  telemetry_selects_validated_https_otlp -- --nocapture
run_exact_test endpoint_rejection \
  cargo test --locked -p worldstream-runtime --lib \
  telemetry_endpoint_rejects_credentials_queries_bad_ports_and_unsupported_schemes -- --nocapture

if [[ -n "$output_path" ]]; then
  python3 - "$output_path" "$timeout_seconds" <<'PY'
import json
import os
import platform
import stat
import sys
import tempfile
from pathlib import Path

destination = Path(sys.argv[1])
destination.parent.mkdir(parents=True, exist_ok=True)
try:
    mode = destination.lstat().st_mode
except FileNotFoundError:
    pass
else:
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise SystemExit(f"telemetry HTTPS smoke: unsafe report path: {destination}")

tests = [
    {
        "id": "trusted_https_otlp",
        "package": "worldstream-server",
        "filter": "tls_transport_uses_trusted_ca_hostname_verification_and_otlp_http",
        "passed_count": 1,
    },
    {
        "id": "tls_rejection",
        "package": "worldstream-server",
        "filter": "tls_transport_rejects_untrusted_cert_and_hostname_mismatch",
        "passed_count": 1,
    },
    {
        "id": "local_otlp_http",
        "package": "worldstream-server",
        "filter": "standard_http_transport_exercises_a_local_otlp_http_collector",
        "passed_count": 1,
    },
    {
        "id": "http_failure_bounds",
        "package": "worldstream-server",
        "filter": "standard_http_transport_rejects_non_success_and_oversized_responses",
        "passed_count": 1,
    },
    {
        "id": "slow_response_bounds",
        "package": "worldstream-server",
        "filter": "standard_http_transport_slow_response_is_bounded",
        "passed_count": 1,
    },
    {
        "id": "https_configuration",
        "package": "worldstream-server/worldstreamd",
        "filter": "telemetry_selects_validated_https_otlp",
        "passed_count": 1,
    },
    {
        "id": "endpoint_rejection",
        "package": "worldstream-runtime",
        "filter": "telemetry_endpoint_rejects_credentials_queries_bad_ports_and_unsupported_schemes",
        "passed_count": 1,
    },
]
report = {
    "schema": "worldstream/telemetry-https-evidence/v1",
    "status": "passed",
    "release_evidence": False,
    "evidence_class": "local_transport_integration",
    "platform": {"system": platform.system(), "machine": platform.machine()},
    "timeout_seconds_per_test": int(sys.argv[2]),
    "checks": {
        "trusted_ca": True,
        "hostname_verification": True,
        "otlp_https_delivery": True,
        "untrusted_ca_rejected": True,
        "hostname_mismatch_rejected": True,
        "non_success_rejected": True,
        "oversized_response_rejected": True,
        "slow_response_bounded": True,
        "endpoint_validation": True,
    },
    "tests": tests,
}
descriptor, temporary_name = tempfile.mkstemp(
    prefix=f".{destination.name}.", dir=destination.parent
)
temporary = Path(temporary_name)
try:
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        json.dump(report, output, ensure_ascii=False, indent=2, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.chmod(temporary, 0o644)
    os.replace(temporary, destination)
except BaseException:
    temporary.unlink(missing_ok=True)
    raise
PY
fi

echo "telemetry HTTPS smoke passed"
