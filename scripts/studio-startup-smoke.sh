#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

require_tool() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "required smoke tool is unavailable: $1" >&2
    exit 1
  }
}

for tool in lsof curl pgrep ps python3 shasum awk grep mktemp cat rm; do
  require_tool "$tool"
done

daemon_port=${WORLDSTREAM_SMOKE_DAEMON_PORT:-19410}
supervisor_port=${WORLDSTREAM_SMOKE_SUPERVISOR_PORT:-19420}
smoke_wait_seconds=${WORLDSTREAM_SMOKE_WAIT_SECONDS:-30}
[[ "$smoke_wait_seconds" =~ ^[1-9][0-9]*$ ]] && ((smoke_wait_seconds <= 120)) || {
  echo "WORLDSTREAM_SMOKE_WAIT_SECONDS must be between 1 and 120" >&2
  exit 1
}
smoke_root=$(mktemp -d)
chmod 700 "$smoke_root"
supervisor_log="$smoke_root/supervisor.log"
supervisor_pid=
owned_daemon_pid=
owned_daemon_identity=
current_stage=initialization
failed_stage=
smoke_succeeded=

set_stage() {
  current_stage=$1
}

on_error() {
  failed_stage=$current_stage
  echo "Studio startup smoke failed during stage: $failed_stage" >&2
}
trap on_error ERR

port_is_available() {
  local port=$1
  [[ "$port" =~ ^[1-9][0-9]{0,4}$ ]] || return 1
  ((port <= 65535)) || return 1
  ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
}

process_identity() {
  local pid=$1
  ps -p "$pid" -o lstart= -o command= 2>/dev/null || true
}

owned_daemon_is_current() {
  [[ -n "$owned_daemon_pid" && -n "$owned_daemon_identity" ]] || return 1
  [[ "$(process_identity "$owned_daemon_pid")" == "$owned_daemon_identity" ]]
}

wait_for_pid_exit() {
  local pid=$1
  for _ in {1..80}; do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  return 1
}

terminate_owned_daemon() {
  if ! owned_daemon_is_current; then
    return 0
  fi
  kill -TERM "$owned_daemon_pid"
  wait_for_pid_exit "$owned_daemon_pid" || {
    echo "owned daemon did not stop" >&2
    return 1
  }
  owned_daemon_pid=
  owned_daemon_identity=
}

cleanup() {
  if [[ -n "$supervisor_pid" ]] && kill -0 "$supervisor_pid" 2>/dev/null; then
    curl --fail --silent --request POST \
      --connect-timeout 1 --max-time 1 \
      "http://127.0.0.1:$supervisor_port/api/v1/daemon/stop" \
      >/dev/null 2>&1 || true
    kill "$supervisor_pid" 2>/dev/null || true
    wait "$supervisor_pid" 2>/dev/null || true
  fi
  supervisor_pid=
  if ! terminate_owned_daemon; then
    echo "preserving smoke state because the owned daemon did not stop" >&2
    return 1
  fi
  if [[ "$smoke_succeeded" != 1 ]]; then
    echo "protected diagnostic evidence retained at: $smoke_root" >&2
    return 0
  fi
  rm -rf "$smoke_root"
}

on_exit() {
  local status=$?
  local cleanup_status=0
  trap - EXIT
  cleanup || cleanup_status=$?
  if (( status != 0 )); then
    exit "$status"
  fi
  if (( cleanup_status != 0 )); then
    exit "$cleanup_status"
  fi
}
trap on_exit EXIT
trap 'exit 130' INT TERM

for executable in \
  target/debug/worldstreamd \
  target/debug/worldstream-studio-supervisor \
  target/debug/worldstream-assignment-mcp; do
  [[ -x "$executable" ]] || {
    echo "build required executable: $executable" >&2
    exit 1
  }
done
port_is_available "$daemon_port" || {
  echo "daemon smoke port is unavailable" >&2
  exit 1
}
port_is_available "$supervisor_port" || {
  echo "Supervisor smoke port is unavailable" >&2
  exit 1
}

launch_supervisor() {
  WORLDSTREAM__SERVER__BIND="127.0.0.1:$daemon_port" \
  WORLDSTREAM__STORAGE__DATA_DIR="$smoke_root/data" \
  WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$smoke_root/authority.secret" \
  target/debug/worldstream-studio-supervisor \
    --bind "127.0.0.1:$supervisor_port" \
    --daemon "127.0.0.1:$daemon_port" \
    --daemon-config config/development.toml \
    --daemon-executable target/debug/worldstreamd \
    --assignment-mcp-executable target/debug/worldstream-assignment-mcp \
    --state-dir "$smoke_root/studio" \
    >>"$supervisor_log" 2>&1 &
  supervisor_pid=$!
}

wait_for() {
  local url=$1
  local channel=$2
  local attempt="$channel.attempt"
  local http_status
  local deadline=$((SECONDS + smoke_wait_seconds))
  while ((SECONDS < deadline)); do
    if ! kill -0 "$supervisor_pid" 2>/dev/null; then
      echo "owned Supervisor exited while waiting during stage: $current_stage" >&2
      return 1
    fi
    http_status=$(curl --silent --show-error --connect-timeout 1 --max-time 1 --output "$attempt" --write-out '%{http_code}' "$url") || http_status=transport_error
    if [[ "$http_status" =~ ^2[0-9][0-9]$ ]]; then
      cat "$attempt" >>"$channel"
      printf '\n' >>"$channel"
      rm -f "$attempt"
      return 0
    fi
    cat "$attempt" >>"$channel" 2>/dev/null || true
    printf '\n' >>"$channel"
    rm -f "$attempt"
    sleep 0.1
  done
  echo "timed out waiting for bounded local endpoint during stage $current_stage (last HTTP status: $http_status)" >&2
  return 1
}

wait_for_running_lifecycle() {
  local channel=$1
  local deadline=$((SECONDS + smoke_wait_seconds))
  while ((SECONDS < deadline)); do
    if ! kill -0 "$supervisor_pid" 2>/dev/null; then
      echo "owned Supervisor exited while waiting for lifecycle" >&2
      return 1
    fi
    local lifecycle
    lifecycle=$(curl --fail --silent --connect-timeout 1 --max-time 1 "http://127.0.0.1:$supervisor_port/api/v1/daemon/lifecycle") || {
      sleep 0.1
      continue
    }
    printf '%s\n' "$lifecycle" >>"$channel"
    if [[ "$lifecycle" == *'"state":"running"'* && "$lifecycle" == *'"managed_by_supervisor":true'* ]]; then
      return 0
    fi
    sleep 0.1
  done
  echo "daemon lifecycle did not reach owned running state" >&2
  return 1
}

record_owned_daemon() {
  local children
  children=$(pgrep -P "$supervisor_pid" || true)
  local matching=()
  local child
  for child in $children; do
    local identity
    identity=$(process_identity "$child")
    if [[ "$identity" == *"target/debug/worldstreamd"* ]]; then
      matching+=("$child")
    fi
  done
  [[ ${#matching[@]} -eq 1 ]] || {
    echo "could not identify exactly one owned daemon process" >&2
    return 1
  }
  owned_daemon_pid=${matching[0]}
  owned_daemon_identity=$(process_identity "$owned_daemon_pid")
  [[ -n "$owned_daemon_identity" ]]
}

scan_secret_absence() {
  python3 - "$repo_root" "$smoke_root" <<'PY'
import importlib.util
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
smoke = Path(sys.argv[2])
spec = importlib.util.spec_from_file_location("secret_scan", root / "scripts" / "verify-secret-absence.py")
module = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(module)
binding = json.loads((smoke / "studio" / "host-authority-reference.json").read_text(encoding="utf-8"))
channels = {
    path.stem.replace("-", ""): path
    for path in sorted(smoke.glob("*.json")) + [smoke / "lifecycle.jsonl", smoke / "supervisor.log"]
}
report = module.scan_sentinels(
    {
        "bootstrapauthority": (smoke / "authority.secret").read_bytes(),
        "hostauthorityreference": binding["reference"].encode("ascii"),
    },
    channels,
)
(smoke / "secret-absence-report.json").write_text(
    json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n",
    encoding="utf-8",
)
print("secret absence scan passed")
PY
}

set_stage "Supervisor launch"
launch_supervisor
set_stage "Supervisor readiness"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/daemon/status" "$smoke_root/status.json"
set_stage "Supervisor secrets status"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/secrets" "$smoke_root/secrets.json"
set_stage "daemon start request"
curl --fail --silent --show-error --request POST \
  --connect-timeout 1 --max-time 1 \
  "http://127.0.0.1:$supervisor_port/api/v1/daemon/start" \
  >"$smoke_root/start.json"
set_stage "owned daemon readiness"
wait_for_running_lifecycle "$smoke_root/lifecycle.jsonl"
record_owned_daemon
daemon_before_restart=$owned_daemon_pid
set_stage "daemon ready endpoint"
wait_for "http://127.0.0.1:$daemon_port/readyz" "$smoke_root/daemon-ready.json"
set_stage "authorized activity-pack catalog"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/activity-packs" "$smoke_root/catalog.json"
set_stage "authorized Room inventory"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/rooms?limit=10" "$smoke_root/rooms.json"
grep -q 'activity_pack_catalog.v1' "$smoke_root/catalog.json"
grep -q 'worldstream/studio-room-inventory/v1' "$smoke_root/rooms.json"

binding_before=$(shasum -a 256 "$smoke_root/studio/host-authority-reference.json" | awk '{print $1}')
set_stage "daemon restart request"
curl --fail --silent --show-error --request POST \
  --connect-timeout 1 --max-time 1 \
  "http://127.0.0.1:$supervisor_port/api/v1/daemon/restart" \
  >"$smoke_root/restart.json"
set_stage "previous daemon termination"
wait_for_pid_exit "$daemon_before_restart" || {
  echo "daemon restart did not reap the previous owned process" >&2
  exit 1
}
set_stage "restarted owned daemon readiness"
wait_for_running_lifecycle "$smoke_root/lifecycle.jsonl"
record_owned_daemon
[[ "$owned_daemon_pid" != "$daemon_before_restart" ]]
set_stage "restarted daemon ready endpoint"
wait_for "http://127.0.0.1:$daemon_port/readyz" "$smoke_root/daemon-ready-after-restart.json"
set_stage "catalog after daemon restart"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/activity-packs" "$smoke_root/catalog-after-daemon-restart.json"
grep -q 'activity_pack_catalog.v1' "$smoke_root/catalog-after-daemon-restart.json"

kill "$supervisor_pid"
wait "$supervisor_pid" || true
supervisor_pid=
set_stage "Supervisor restart launch"
launch_supervisor
set_stage "Supervisor restart readiness"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/daemon/status" "$smoke_root/status-after-supervisor-restart.json"
set_stage "catalog after Supervisor restart"
wait_for "http://127.0.0.1:$supervisor_port/api/v1/activity-packs" "$smoke_root/catalog-after-supervisor-restart.json"
grep -q 'activity_pack_catalog.v1' "$smoke_root/catalog-after-supervisor-restart.json"
binding_after=$(shasum -a 256 "$smoke_root/studio/host-authority-reference.json" | awk '{print $1}')
[[ "$binding_before" == "$binding_after" ]]
set_stage "secret non-disclosure scan"
scan_secret_absence

smoke_succeeded=1
if ! cleanup; then
  trap - EXIT INT TERM
  exit 1
fi
trap - EXIT INT TERM
echo "Studio startup smoke passed"
