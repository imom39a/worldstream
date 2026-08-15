#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

smoke_root="$(mktemp -d)"
data_dir="$smoke_root/data"
server_log="$smoke_root/worldstreamd.log"
if [[ -n "${WORLDSTREAM_SMOKE_PORT:-}" ]]; then
  smoke_port="$WORLDSTREAM_SMOKE_PORT"
else
  smoke_port="$(python3 - <<'PY'
import socket

with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)"
fi
bind_address="127.0.0.1:$smoke_port"
base_url="http://$bind_address"
server_pid=""

stop_server() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" >/dev/null 2>&1; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$smoke_root"
}
trap stop_server EXIT

target/debug/worldstreamctl --data-dir "$data_dir" config effective \
  | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["server"]["bind"] == "127.0.0.1:9410"'
target/debug/worldstreamctl --data-dir "$data_dir" doctor \
  | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["data_directory"] == "owner_only"; assert value["storage"] == "not_initialized"'

RUST_LOG=info target/debug/worldstreamd --data-dir "$data_dir" --bind "$bind_address" >"$server_log" 2>&1 &
server_pid="$!"

for _ in {1..50}; do
  if curl -fsS "$base_url/healthz" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$server_pid" >/dev/null 2>&1; then
    sed -n '1,120p' "$server_log" >&2
    exit 1
  fi
  sleep 0.1
done

if ! kill -0 "$server_pid" >/dev/null 2>&1; then
  sed -n '1,120p' "$server_log" >&2
  exit 1
fi
if ! grep -Fq "\"listen_address\":\"$bind_address\"" "$server_log"; then
  printf '%s\n' 'worldstreamd did not emit this smoke run startup event' >&2
  sed -n '1,120p' "$server_log" >&2
  exit 1
fi

health_status="$(curl -sS -o "$smoke_root/health.json" -w '%{http_code}' "$base_url/healthz")"
ready_status="$(curl -sS -o "$smoke_root/ready.json" -w '%{http_code}' "$base_url/readyz")"
version_status="$(curl -sS -o "$smoke_root/version.json" -w '%{http_code}' "$base_url/version")"

[[ "$health_status" == "200" ]]
[[ "$ready_status" == "503" ]]
[[ "$version_status" == "200" ]]

python3 - "$smoke_root/ready.json" "$smoke_root/version.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    ready = json.load(source)
with open(sys.argv[2], encoding="utf-8") as source:
    version = json.load(source)

assert ready["error"]["code"] == "storage_not_initialized"
assert version["manifest"]["release_ready"] is False
assert version["engine"]["status"] == "not_initialized"
assert version["engine"]["exact_identity"] is None
PY

kill -0 "$server_pid"
grep -Fq "\"listen_address\":\"$bind_address\"" "$server_log"

printf '%s\n' 'WorldStream operator-shell smoke passed.'
