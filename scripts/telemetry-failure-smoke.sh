#!/usr/bin/env bash
set -euo pipefail

# IMO-58 process-level telemetry failure witness. It exercises the bounded
# optional OTLP/HTTP worker, malformed-config fallback, and the canonical
# projection before/after telemetry pressure. It makes no release claim.

for command_name in curl python3 mktemp stat; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'telemetry failure smoke: required command is unavailable: %s\n' "$command_name" >&2
    exit 2
  fi
done

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
daemon_bin="${WORLDSTREAMD_BIN:-$workspace_dir/target/debug/worldstreamd}"
if [[ ! -x "$daemon_bin" ]]; then
  printf 'telemetry failure smoke: daemon is not executable: %s\n' "$daemon_bin" >&2
  exit 2
fi

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-telemetry-failure.XXXXXX")"
chmod 700 "$smoke_root"
data_dir="$smoke_root/data"
bootstrap_secret="$smoke_root/authority.secret"
server_log="$smoke_root/worldstreamd.log"
invalid_log="$smoke_root/invalid-config.log"
mkdir -m 700 "$data_dir"
python3 - "$bootstrap_secret" <<'PY'
import os
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_bytes(os.urandom(32))
path.chmod(0o600)
PY
server_pid=""
invalid_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" >/dev/null 2>&1; then
    kill -TERM "$server_pid" >/dev/null 2>&1 || true
    for _ in {1..100}; do
      if ! kill -0 "$server_pid" >/dev/null 2>&1; then
        break
      fi
      sleep 0.05
    done
    kill -KILL "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$invalid_pid" ]] && kill -0 "$invalid_pid" >/dev/null 2>&1; then
    kill -TERM "$invalid_pid" >/dev/null 2>&1 || true
    wait "$invalid_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$smoke_root"
}
trap cleanup EXIT

next_port() {
  python3 - <<'PY'
import socket

with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
}

smoke_port="$(next_port)"
bind_address="127.0.0.1:$smoke_port"
base_url="http://$bind_address"

# Select a syntactically valid but unreachable collector. The worker must
# discard that outage asynchronously without changing the durable response.
WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT="http://127.0.0.1:1/v1/logs" \
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" \
RUST_LOG=info "$daemon_bin" --data-dir "$data_dir" --bind "$bind_address" >"$server_log" 2>&1 &
server_pid="$!"

for _ in {1..120}; do
  if [[ "$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$base_url/healthz" 2>/dev/null || true)" == "200" ]]; then
    break
  fi
  if ! kill -0 "$server_pid" >/dev/null 2>&1; then
    sed "s|$smoke_root|<temp>|g" "$server_log" >&2
    printf '%s\n' 'telemetry failure smoke: daemon exited before /healthz' >&2
    exit 1
  fi
  sleep 0.05
done

if [[ "$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$base_url/healthz" 2>/dev/null || true)" != "200" ]]; then
  sed "s|$smoke_root|<temp>|g" "$server_log" >&2
  printf '%s\n' 'telemetry failure smoke: /healthz did not become live' >&2
  exit 1
fi

host_bearer="wsb1:$(od -An -tx1 -v "$bootstrap_secret" | tr -d ' \n')"
host_curl_config="$smoke_root/host.curl"
printf 'header = "Authorization: Bearer %s"\n' "$host_bearer" >"$host_curl_config"
chmod 600 "$host_curl_config"
pack_digest="blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92"
create_idempotency_id="01ARZ3NDEKTSV4RRFFQ69G5FE0"
cat >"$smoke_root/create.json" <<EOF
{"pack":{"id":"worldstream.counter","version":"2.0.0","digest":"$pack_digest"},"configuration":{"initial_value":0,"maximum_value":16},"members":[{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FC2","principal_kind":"human","role":"counter","access_mode":"participant"}],"idempotency_key":"$create_idempotency_id"}
EOF

probe() {
  local path="$1"
  local output="$2"
  local expected="$3"
  local status
  status="$(curl -sS --connect-timeout 1 --max-time 5 --config "$host_curl_config" -H 'Content-Type: application/json' -o "$output" -w '%{http_code}' "$base_url$path" 2>/dev/null || true)"
  [[ "$status" == "$expected" ]] || {
    printf 'telemetry failure smoke: %s returned HTTP %s; expected %s\n' "$path" "$status" "$expected" >&2
    exit 1
  }
}

probe "/readyz" "$smoke_root/ready-before.json" 200
probe "/metrics" "$smoke_root/metrics-before.txt" 200
probe "/version" "$smoke_root/version.json" 200

create_status="$(curl -sS --connect-timeout 1 --max-time 5 \
  --config "$host_curl_config" \
  -H 'Content-Type: application/json' \
  --data-binary "@$smoke_root/create.json" \
  -o "$smoke_root/create-first.json" -w '%{http_code}' "$base_url/v1/rooms" 2>/dev/null || true)"
[[ "$create_status" == "200" ]] || {
  printf 'telemetry failure smoke: Room create returned HTTP %s\n' "$create_status" >&2
  sed "s|$smoke_root|<temp>|g" "$server_log" >&2
  exit 1
}

room_id="$(python3 - "$smoke_root/create-first.json" <<'PY'
import json
import pathlib
import sys

print(json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["room_id"])
PY
)"
member_id="$(python3 - "$smoke_root/create-first.json" <<'PY'
import json
import pathlib
import sys

print(json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["member_ids"][0])
PY
)"
capability_idempotency_id="01ARZ3NDEKTSV4RRFFQ69G5FF0"
cat >"$smoke_root/member-capability.json" <<EOF
{"room_id":"$room_id","member_id":"$member_id","principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FC2","scopes":["room:observe_member","room:replay"],"idempotency_key":"$capability_idempotency_id"}
EOF
capability_status="$(curl -sS --connect-timeout 1 --max-time 5 \
  --config "$host_curl_config" \
  -H 'Content-Type: application/json' \
  --data-binary "@$smoke_root/member-capability.json" \
  -o "$smoke_root/member-capability-response.json" -w '%{http_code}' "$base_url/v1/operator/member-capabilities" 2>/dev/null || true)"
[[ "$capability_status" == "200" ]] || {
  printf 'telemetry failure smoke: member capability issuance returned HTTP %s\n' "$capability_status" >&2
  exit 1
}
member_bearer="$(python3 - "$smoke_root/member-capability-response.json" <<'PY'
import json
import pathlib
import sys

print(json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))["bearer"])
PY
)"
member_curl_config="$smoke_root/member.curl"
printf 'header = "Authorization: Bearer %s"\n' "$member_bearer" >"$member_curl_config"
chmod 600 "$member_curl_config"
probe_member() {
  local path="$1"
  local output="$2"
  local expected="$3"
  local status
  status="$(curl -sS --connect-timeout 1 --max-time 5 --config "$member_curl_config" -o "$output" -w '%{http_code}' "$base_url$path" 2>/dev/null || true)"
  [[ "$status" == "$expected" ]] || {
    printf 'telemetry failure smoke: member %s returned HTTP %s; expected %s\n' "$path" "$status" "$expected" >&2
    exit 1
  }
}
probe_member "/v1/rooms/$room_id/projection" "$smoke_root/projection-before.json" 200

retry_status="$(curl -sS --connect-timeout 1 --max-time 5 \
  --config "$host_curl_config" \
  -H 'Content-Type: application/json' \
  --data-binary "@$smoke_root/create.json" \
  -o "$smoke_root/create-retry.json" -w '%{http_code}' "$base_url/v1/rooms" 2>/dev/null || true)"
[[ "$retry_status" == "200" ]] || {
  printf 'telemetry failure smoke: idempotent Room retry returned HTTP %s\n' "$retry_status" >&2
  exit 1
}

# Generate bounded post-commit/unauthorized telemetry pressure using only the
# public route.  Whether the fixed 256-slot queue drops anything is observed,
# never required as a success condition.
python3 - "$base_url" "$smoke_root" <<'PY'
from concurrent.futures import ThreadPoolExecutor
import json
import pathlib
import sys
import urllib.error
import urllib.request

base = sys.argv[1]
root = pathlib.Path(sys.argv[2])
payload = b'{"not":"a valid room request"}'

def one(index: int) -> int:
    request = urllib.request.Request(
        f"{base}/v1/rooms",
        data=payload,
        method="POST",
        headers={
            "Authorization": f"Bearer wsb1:{'ab' * 32}",
            "Content-Type": "application/json",
            "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "X-Pressure-Index": str(index),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code

with ThreadPoolExecutor(max_workers=32) as executor:
    statuses = list(executor.map(one, range(384)))

assert all(status == 400 or status == 403 for status in statuses), sorted(set(statuses))
(root / "pressure.json").write_text(
    json.dumps({"request_count": len(statuses), "statuses": sorted(set(statuses))}),
    encoding="utf-8",
)
PY

for _ in {1..40}; do
  curl -sS --connect-timeout 1 --max-time 2 "$base_url/metrics" >"$smoke_root/metrics-after.txt"
  if grep -Eq '^worldstream_telemetry_exporter_failures_total [1-9][0-9]*$' "$smoke_root/metrics-after.txt"; then
    break
  fi
  sleep 0.05
done
probe "/readyz" "$smoke_root/ready-after.json" 200
probe_member "/v1/rooms/$room_id/projection" "$smoke_root/projection-after.json" 200

# Malformed telemetry is a typed diagnostic, not a process-fatal config error.
# The endpoint text (which may contain credentials/query material) must never
# enter logs or redacted diagnostics.
invalid_port="$(next_port)"
invalid_base_url="http://127.0.0.1:$invalid_port"
WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT="not-a-url-telemetry-secret-marker" \
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" \
RUST_LOG=info "$daemon_bin" --data-dir "$smoke_root/invalid-data" --bind "127.0.0.1:$invalid_port" >"$invalid_log" 2>&1 &
invalid_pid="$!"
for _ in {1..120}; do
  if [[ "$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$invalid_base_url/healthz" 2>/dev/null || true)" == "200" ]]; then
    break
  fi
  if ! kill -0 "$invalid_pid" >/dev/null 2>&1; then
    sed "s|$smoke_root|<temp>|g" "$invalid_log" >&2
    printf '%s\n' 'telemetry failure smoke: malformed config stopped the daemon' >&2
    exit 1
  fi
  sleep 0.05
done
[[ "$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$invalid_base_url/healthz" 2>/dev/null || true)" == "200" ]] || {
  sed "s|$smoke_root|<temp>|g" "$invalid_log" >&2
  printf '%s\n' 'telemetry failure smoke: malformed config fallback did not become live' >&2
  exit 1
}
kill -TERM "$invalid_pid" >/dev/null 2>&1 || true
wait "$invalid_pid" >/dev/null 2>&1 || true

python3 - "$smoke_root" "$server_log" "$invalid_log" <<'PY'
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
server_log = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8", errors="replace")
invalid_log = pathlib.Path(sys.argv[3]).read_text(encoding="utf-8", errors="replace")
first = json.loads((root / "create-first.json").read_text(encoding="utf-8"))
retry = json.loads((root / "create-retry.json").read_text(encoding="utf-8"))
before = (root / "metrics-before.txt").read_text(encoding="utf-8")
after = (root / "metrics-after.txt").read_text(encoding="utf-8")
ready_before = json.loads((root / "ready-before.json").read_text(encoding="utf-8"))
ready_after = json.loads((root / "ready-after.json").read_text(encoding="utf-8"))
version = json.loads((root / "version.json").read_text(encoding="utf-8"))
pressure = json.loads((root / "pressure.json").read_text(encoding="utf-8"))

assert first == retry, "idempotent retry changed the committed Room response"
assert json.loads((root / "projection-before.json").read_text(encoding="utf-8")) == json.loads((root / "projection-after.json").read_text(encoding="utf-8")), "telemetry pressure changed the canonical projection"
assert first["room_head"]["room_seq"] == 0
assert first["room_head"]["authoritative_state_hash"]
assert ready_before == {"status": "ready"}
assert ready_after == {"status": "ready"}
assert version["manifest"]["release_ready"] is True
assert version["engine"]["status"] == "verified"
assert "worldstream_telemetry_enqueued_total" in before
assert "worldstream_telemetry_exporter_failures_total" in after
assert re.search(r'^worldstream_telemetry_events_total\{event="migration"\} [1-9][0-9]*$', before, re.MULTILINE), "SQLite migration producer did not emit a migration event"
assert "room_id" not in after
assert "telemetry-secret-marker" not in server_log
assert "Bearer" not in server_log
assert "not-a-url-telemetry-secret-marker" not in invalid_log
assert '"event":"storage_diagnostic"' in invalid_log
assert '"reason":"exporter_malformed_endpoint"' in invalid_log
assert pressure["request_count"] == 384

def metric(text: str, name: str) -> int:
    match = re.search(rf"^{re.escape(name)} ([0-9]+)$", text, re.MULTILINE)
    assert match, name
    return int(match.group(1))

def metric_label(text: str, event: str) -> int:
    match = re.search(
        rf'^worldstream_telemetry_events_total\{{event="{re.escape(event)}"\}} ([0-9]+)$',
        text,
        re.MULTILINE,
    )
    assert match, event
    return int(match.group(1))

result = {
    "status": "passed",
    "evidence_class": "real_disposable_daemon_http_process",
    "release_evidence": False,
    "secrets": "not_emitted",
    "otlp": {
        "standard_unreachable_endpoint_supplied": True,
        "runtime_otlp_hook": "bounded_plain_http_worker",
        "collector_delivery": "not_claimed",
        "malformed_worldstream_config_fallback": True,
        "invalid_config_exit_nonzero": False,
    },
    "room": {
        "committed": True,
        "idempotent_retry_same_response": first == retry,
        "room_seq": first["room_head"]["room_seq"],
        "authoritative_state_hash": first["room_head"]["authoritative_state_hash"],
    },
    "readiness": {
        "before": ready_before["status"],
        "after": ready_after["status"],
        "unaffected_by_telemetry_failure": True,
    },
    "pressure": {
        "requests": pressure["request_count"],
        "statuses": pressure["statuses"],
        "queue_saturation": "not_observable_as_a_required_outcome_through_public_daemon_hooks",
        "process_remained_ready": True,
    },
    "metrics": {
        "enqueued_before": metric(before, "worldstream_telemetry_enqueued_total"),
        "enqueued_after": metric(after, "worldstream_telemetry_enqueued_total"),
        "dropped_after": metric(after, "worldstream_telemetry_dropped_total"),
        "exporter_failures_after": metric(after, "worldstream_telemetry_exporter_failures_total"),
        "sqlite_migration_events_before": metric_label(before, "migration"),
    },
    "redaction": {
        "authorization_and_markers_absent_from_logs": True,
        "high_cardinality_room_id_absent_from_metrics": True,
    },
}
print(json.dumps(result, sort_keys=True, separators=(",", ":")))
PY

printf '%s\n' 'WorldStream telemetry failure smoke passed: committed Room/hash and readiness survived bounded telemetry-failure pressure; malformed configuration fell back without retaining its endpoint.'
