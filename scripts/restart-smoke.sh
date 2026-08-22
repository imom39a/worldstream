#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  printf '%s\n' 'restart smoke: Linux is required' >&2
  exit 2
fi

for command_name in curl python3 mktemp stat ps; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'restart smoke: required command is unavailable: %s\n' "$command_name" >&2
    exit 2
  fi
done

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
daemon_bin="${WORLDSTREAMD_BIN:-$workspace_dir/target/debug/worldstreamd}"
if [[ ! -x "$daemon_bin" ]]; then
  printf 'restart smoke: daemon is not executable: %s\n' "$daemon_bin" >&2
  exit 2
fi

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-restart-smoke.XXXXXX")"
chmod 700 "$smoke_root"
data_dir="$smoke_root/data"
bootstrap_secret="$smoke_root/authority.secret"
mkdir -m 700 "$data_dir"
dd if=/dev/zero of="$bootstrap_secret" bs=32 count=1 status=none
chmod 600 "$bootstrap_secret"
server_pid=""

redacted_log() {
  local log_file="$1"
  if [[ -f "$log_file" ]]; then
    sed "s|$smoke_root|<temp>|g" "$log_file" | tail -n 40 >&2
  fi
}

process_alive() {
  [[ -n "$server_pid" ]] || return 1
  kill -0 "$server_pid" >/dev/null 2>&1 || return 1
  local process_state
  process_state="$(ps -o stat= -p "$server_pid" 2>/dev/null || true)"
  case "$process_state" in
    *Z*) return 1 ;;
  esac
  return 0
}

cleanup() {
  if process_alive; then
    kill -TERM "$server_pid" >/dev/null 2>&1 || true
    for _ in {1..20}; do
      if ! process_alive; then
        break
      fi
      sleep 0.05
    done
    if process_alive; then
      kill -KILL "$server_pid" >/dev/null 2>&1 || true
    fi
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$smoke_root"
}
trap cleanup EXIT

fail() {
  printf 'restart smoke: FAIL: %s\n' "$1" >&2
  if [[ -n "${current_log:-}" ]]; then
    redacted_log "$current_log"
  fi
  exit 1
}

next_port() {
  python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
}

if [[ -n "${WORLDSTREAM_SMOKE_PORT:-}" ]]; then
  smoke_port="$WORLDSTREAM_SMOKE_PORT"
else
  smoke_port="$(next_port)"
fi
if [[ ! "$smoke_port" =~ ^[0-9]+$ ]] || (( smoke_port < 1 || smoke_port > 65535 )); then
  printf 'restart smoke: invalid WORLDSTREAM_SMOKE_PORT: %s\n' "$smoke_port" >&2
  exit 2
fi

start_server() {
  local bind_address="$1"
  local log_file="$2"
  current_log="$log_file"
  WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" \
    RUST_LOG=info "$daemon_bin" --data-dir "$data_dir" --bind "$bind_address" >"$log_file" 2>&1 &
  server_pid="$!"
}

wait_for_health() {
  local base_url="$1"
  local log_file="$2"
  current_log="$log_file"
  for _ in {1..100}; do
    if [[ "$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$base_url/healthz" 2>/dev/null || true)" == "200" ]]; then
      return 0
    fi
    if ! process_alive; then
      fail 'worldstreamd exited before /healthz became live'
    fi
    sleep 0.1
  done
  fail '/healthz did not become live within the bounded startup window'
}

probe_json() {
  local url="$1"
  local output_file="$2"
  local expected_status="$3"
  local actual_status
  actual_status="$(curl -sS --connect-timeout 1 --max-time 2 -o "$output_file" -w '%{http_code}' "$url" 2>/dev/null || true)"
  [[ "$actual_status" == "$expected_status" ]] || fail "$url returned HTTP $actual_status; expected HTTP $expected_status"
}

probe_first_run() {
  local base_url="$1"
  probe_json "$base_url/healthz" "$smoke_root/health.json" 200
  probe_json "$base_url/readyz" "$smoke_root/ready.json" 200
  probe_json "$base_url/version" "$smoke_root/version.json" 200

  python3 - "$smoke_root/ready.json" "$smoke_root/version.json" <<'PY'
import json
import sys

ready = json.loads(open(sys.argv[1], encoding="utf-8").read())
version = json.loads(open(sys.argv[2], encoding="utf-8").read())
assert ready["status"] == "ready"
assert version["manifest"]["release_ready"] is True
assert version["engine"]["status"] == "verified"
assert version["engine"]["exact_identity"]
PY

  local room_status
  room_status="$(curl -sS --connect-timeout 1 --max-time 2 \
    -H 'Authorization: Bearer wsb1:1111111111111111111111111111111111111111111111111111111111111111' \
    -H 'Content-Type: application/json' \
    --data '{"pack":{"id":"worldstream.counter","version":"1","digest":"blake3:0000000000000000000000000000000000000000000000000000000000000000"},"configuration":{},"members":[],"idempotency_key":"restart-smoke-fixture"}' \
    -o "$smoke_root/create-room.json" -w '%{http_code}' "$base_url/v1/rooms" 2>/dev/null || true)"
  [[ "$room_status" != 2* ]] || fail 'Room creation returned success without a verified authority/backend result'
  python3 - "$room_status" "$smoke_root/create-room.json" <<'PY'
import json
import sys

status = int(sys.argv[1])
body = json.loads(open(sys.argv[2], encoding="utf-8").read())
assert status == 403, (status, body)
assert body["error"]["code"] == "forbidden"
PY
}

terminate_server() {
  local log_file="$1"
  current_log="$log_file"
  kill -TERM "$server_pid" 2>/dev/null || fail 'could not send TERM to worldstreamd'
  for _ in {1..100}; do
    if ! process_alive; then
      wait "$server_pid" >/dev/null 2>&1 || true
      server_pid=""
      return 0
    fi
    sleep 0.05
  done
  fail 'worldstreamd did not exit after TERM within five seconds'
}

bind_address="127.0.0.1:$smoke_port"
base_url="http://$bind_address"
start_server "$bind_address" "$smoke_root/first.log"
wait_for_health "$base_url" "$smoke_root/first.log"
grep -Fq "\"listen_address\":\"$bind_address\"" "$smoke_root/first.log" \
  || fail 'startup log did not identify this listener'
probe_first_run "$base_url"

database_path="$data_dir/worldstream.sqlite3"
[[ -s "$database_path" ]] || fail 'SQLite database file was not created'
database_size_before="$(stat -c '%s' "$database_path")"
database_inode_before="$(stat -c '%i' "$database_path")"
terminate_server "$smoke_root/first.log"

start_server "$bind_address" "$smoke_root/second.log"
wait_for_health "$base_url" "$smoke_root/second.log"
grep -Fq "\"listen_address\":\"$bind_address\"" "$smoke_root/second.log" \
  || fail 'restart log did not identify this listener'
probe_json "$base_url/readyz" "$smoke_root/restart-ready.json" 200
probe_json "$base_url/version" "$smoke_root/restart-version.json" 200
python3 - "$smoke_root/restart-ready.json" "$smoke_root/restart-version.json" <<'PY'
import json
import sys

ready = json.loads(open(sys.argv[1], encoding="utf-8").read())
version = json.loads(open(sys.argv[2], encoding="utf-8").read())
assert ready["status"] == "ready"
assert version["manifest"]["release_ready"] is True
assert version["engine"]["status"] == "verified"
assert version["engine"]["exact_identity"]
PY
database_size_after="$(stat -c '%s' "$database_path")"
database_inode_after="$(stat -c '%i' "$database_path")"
[[ "$database_inode_before" == "$database_inode_after" ]] \
  || fail 'restart did not reopen the same SQLite database file'
(( database_size_after >= database_size_before )) \
  || fail 'SQLite database shrank across the bounded restart observation'
terminate_server "$smoke_root/second.log"

fault_path="$smoke_root/not-a-directory"
: >"$fault_path"
fault_port="$(next_port)"
fault_url="http://127.0.0.1:$fault_port"
current_log="$smoke_root/fault.log"
RUST_LOG=info "$daemon_bin" --data-dir "$fault_path" --bind "127.0.0.1:$fault_port" >"$current_log" 2>&1 &
server_pid="$!"
for _ in {1..40}; do
  if ! kill -0 "$server_pid" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
if process_alive; then
  fail 'daemon remained alive with an invalid storage path'
fi
fault_exit=0
wait "$server_pid" || fault_exit="$?"
server_pid=""
[[ "$fault_exit" != "0" ]] || fail 'daemon exited successfully despite invalid storage'
fault_status="$(curl -sS --connect-timeout 1 --max-time 2 -o /dev/null -w '%{http_code}' "$fault_url/readyz" 2>/dev/null || true)"
[[ "$fault_status" != "200" ]] || fail 'invalid storage path exposed a successful readiness response'

printf '%s\n' 'WorldStream restart smoke passed: health/version/readiness observed, SQLite file survived TERM/restart, invalid storage did not serve.'
printf '%s\n' 'The valid owner-only bootstrap secret makes this daemon ready; the synthetic bearer remains forbidden and no Room success is claimed by this smoke.'
