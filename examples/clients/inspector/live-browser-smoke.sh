#!/usr/bin/env bash
set -euo pipefail

# Real-browser acceptance smoke for IMO-56/57. This deliberately requires a
# cmux browser surface; headless DOM fixtures are not accepted as evidence.

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$workspace_dir"
exec "$workspace_dir/web/console/live-browser-story.sh"

blocked() {
  printf 'blocked:%s\n' "$1" >&2
  exit 2
}

command -v cmux >/dev/null 2>&1 || blocked browser_cli_unavailable
[[ -x target/debug/worldstreamd ]] || blocked worldstreamd_binary_missing
[[ -d examples/clients/inspector/dist ]] || blocked production_console_build_missing
python_bin="${WORLDSTREAM_PYTHON:-$workspace_dir/sdk/python/.venv/bin/python}"
[[ -x "$python_bin" ]] || blocked python_sdk_runtime_missing

workspace_ref="${CMUX_WORKSPACE_ID:-}"
[[ -n "$workspace_ref" ]] || blocked cmux_workspace_context_missing

root="$(mktemp -d -t worldstream-live-browser.XXXXXX)"
chmod 700 "$root"
server_pid=""
console_pid=""
driver_pid=""
surface=""
preserve_artifacts="${KEEP_LIVE_BROWSER_ARTIFACTS:-0}"
window_ref="${CMUX_WINDOW_ID:-$(cmux identify --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["caller"]["window_ref"])')}"

cleanup() {
  if [[ -n "$surface" ]]; then
    cmux close-surface --surface "$surface" --workspace "$workspace_ref" --window "$window_ref" >/dev/null 2>&1 || true
  fi
  if [[ -n "$console_pid" ]] && kill -0 "$console_pid" >/dev/null 2>&1; then
    kill "$console_pid" >/dev/null 2>&1 || true
    wait "$console_pid" 2>/dev/null || true
  fi
  if [[ -n "$driver_pid" ]] && kill -0 "$driver_pid" >/dev/null 2>&1; then
    kill "$driver_pid" >/dev/null 2>&1 || true
    wait "$driver_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" >/dev/null 2>&1; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [[ "$preserve_artifacts" = 1 ]]; then
    rm -f "$root/authority.secret" "$root/operator.bearer" "$root/browser-manifest.json" "$root/driver-manifest.json"
    rm -rf "$root/data"
    printf 'evidence_root=%s\n' "$root" >&2
  else
    rm -rf "$root"
  fi
}
trap cleanup EXIT

data_dir="$root/data"
mkdir -m 700 "$data_dir"
bootstrap_secret="$root/authority.secret"
operator_bearer="$root/operator.bearer"
manifest="$root/browser-manifest.json"
driver_manifest="$root/driver-manifest.json"
python3 - "$bootstrap_secret" "$operator_bearer" <<'PY'
import os
import secrets
import sys

secret_path, bearer_path = sys.argv[1:]
secret = secrets.token_bytes(32)
fd = os.open(secret_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "wb") as handle:
    handle.write(secret)
    handle.flush()
    os.fsync(handle.fileno())
with open(bearer_path, "x", encoding="utf-8") as handle:
    handle.write("wsb1:" + secret.hex() + "\n")
os.chmod(bearer_path, 0o600)
PY

port="$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)"
base_url="http://127.0.0.1:$port"
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" \
  RUST_LOG="${WORLDSTREAM_LIVE_RUST_LOG:-warn}" target/debug/worldstreamd --data-dir "$data_dir" --bind "127.0.0.1:$port" \
  >"$root/worldstreamd.log" 2>&1 &
server_pid="$!"

ready=0
for _ in $(seq 1 150); do
  if curl -fsS "$base_url/readyz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  kill -0 "$server_pid" >/dev/null 2>&1 || blocked daemon_exited_before_ready
  sleep 0.1
done
[[ "$ready" = 1 ]] || blocked daemon_ready_timeout

PYTHONPATH="$workspace_dir/sdk/python/src:$workspace_dir/examples/heist/wave10_live" \
  "$python_bin" examples/heist/wave10_live/seed_browser_room.py \
    --base-url "$base_url" \
    --operator-bearer-file "$operator_bearer" \
    --output "$manifest" \
    --driver-output "$driver_manifest" >"$root/seed-summary.txt"

console_port="$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)"
python3 -m http.server "$console_port" --bind 127.0.0.1 --directory examples/clients/inspector/dist \
  >"$root/console-http.log" 2>&1 &
console_pid="$!"
console_url="http://127.0.0.1:$console_port/?view=participant"

browser_open="$root/browser-open.json"
cmux --json browser open about:blank --workspace "$workspace_ref" --window "$window_ref" --focus false >"$browser_open" \
  || blocked browser_surface_create_failed
surface="$(python3 - "$browser_open" <<'PY'
import json
import sys

value = json.load(open(sys.argv[1], encoding="utf-8"))
surface = value.get("surface_ref")
if not isinstance(surface, str) or not surface.startswith("surface:"):
    raise SystemExit("browser_surface_ref_missing")
print(surface)
PY
)" || blocked browser_surface_ref_missing

init_script="$(python3 - "$manifest" <<'PY'
import json
import sys

config = json.load(open(sys.argv[1], encoding="utf-8"))
print("window.__WORLDSTREAM_LIVE_SESSION__=" + json.dumps(config, separators=(",", ":")) + ";" + r'''window.__WORLDSTREAM_BROWSER_TRACE__=[];window.__WORLDSTREAM_HOLD_OBSERVATIONS__=false;window.__WORLDSTREAM_OBSERVATION_QUEUE__=[];
(function(){
  const trace=window.__WORLDSTREAM_BROWSER_TRACE__;
  const originalFetch=window.fetch.bind(window);
  window.fetch=async function(input,init){
    const headers=new Headers(init&&init.headers);
    trace.push({kind:"fetch",url:String(input),method:(init&&init.method)||"GET",has_authorization:headers.has("authorization")});
    const response=await originalFetch(input,init);
    trace.push({kind:"fetch-response",url:String(input),status:response.status});
    return response;
  };
  const NativeWebSocket=window.WebSocket;
  function TracedWebSocket(url,protocols){
    const socket=new NativeWebSocket(url,protocols);
    let handler=null;
    const proxy=new Proxy(socket,{get(target,property){if(property==="onmessage")return handler;return Reflect.get(target,property,target);},set(target,property,value){if(property==="onmessage"){handler=value;target.onmessage=function(event){let type=null;try{type=typeof event.data==="string"?JSON.parse(event.data).type:null;}catch{}if(type==="observation.deliver"&&window.__WORLDSTREAM_HOLD_OBSERVATIONS__){window.__WORLDSTREAM_OBSERVATION_QUEUE__.push(event);return;}if(typeof handler==="function")handler.call(proxy,event);};return true;}return Reflect.set(target,property,value);}});
    trace.push({kind:"websocket",url:String(url),protocols:Array.isArray(protocols)?protocols:[protocols]});
    const send=socket.send.bind(socket);
    proxy.send=function(data){
      let type=null;
      try{type=typeof data==="string"?JSON.parse(data).type:null;}catch{}
      trace.push({kind:"send",ticket_first:typeof data==="string" && /^wst1:[0-9a-f]{64}$/.test(data),type});
      return send(data);
    };
    return proxy;
  }
  TracedWebSocket.prototype=NativeWebSocket.prototype;
  Object.setPrototypeOf(TracedWebSocket,NativeWebSocket);
  window.WebSocket=TracedWebSocket;
})();''')
PY
)"
cmux browser "$surface" addinitscript --script "$init_script" >"$root/browser-init.txt" \
  || blocked browser_init_script_failed
cmux browser "$surface" navigate "$console_url" >"$root/browser-navigate.txt" \
  || blocked browser_navigation_failed
cmux browser "$surface" wait --load-state complete --timeout-ms 15000 >"$root/browser-load.txt" \
  || blocked browser_page_load_timeout
cmux browser "$surface" wait --text "Live session · live" --timeout-ms 30000 >"$root/browser-live.txt" \
  || blocked live_session_did_not_reach_live
printf '%s\n' "$(cmux browser "$surface" eval --script 'String(window.__WORLDSTREAM_LIVE_SESSION__)')" \
  | rg -q 'undefined' || blocked ephemeral_config_not_cleared_at_start

before_text="$root/before-action.txt"
after_text="$root/after-action.txt"
cmux browser "$surface" get text body >"$before_text"
rg -q "Authenticated session established|Room synchronization acknowledged|Live session" "$before_text" \
  || blocked live_lifecycle_not_visible
cmux browser "$surface" fill 'input[name="clue_id"]' route >"$root/action-fill.txt" \
  || blocked inspect_clue_input_missing
cmux browser "$surface" click 'button[type="submit"]' >"$root/action-click.txt" \
  || blocked action_submit_control_missing
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 \
  >"$root/action-accepted.txt" || blocked authorized_action_not_accepted
cmux browser "$surface" get text body >"$after_text"

url_text="$(cmux browser "$surface" get url)"
page_html="$(cmux browser "$surface" get html body)"
console_text="$(cmux browser "$surface" console list 2>/dev/null || true)"
errors_text="$(cmux browser "$surface" errors list 2>/dev/null || true)"
local_storage="$(cmux browser "$surface" storage local get 2>/dev/null || true)"
session_storage="$(cmux browser "$surface" storage session get 2>/dev/null || true)"
trace_text="$(cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])')"

printf '%s\n' "$url_text" "$page_html" "$trace_text" "$console_text" "$errors_text" "$local_storage" "$session_storage" \
  | rg -q 'wsb1:|wst1:' && blocked browser_secret_or_ticket_leaked
printf '%s\n' "$url_text" | rg -q '127\.0\.0\.1:[0-9]+/\?view=participant$' \
  || blocked unexpected_browser_url
printf '%s\n' "$trace_text" | rg -q '"kind":"fetch".*v1/stream/ticket.*"has_authorization":true' \
  || blocked ticket_request_not_observed
printf '%s\n' "$trace_text" | rg -q '"kind":"websocket".*v1/stream.*worldstream.json.v0.1' \
  || blocked websocket_request_not_observed
printf '%s\n' "$trace_text" | rg -q '"kind":"send".*"ticket_first":true' \
  || blocked ticket_first_frame_not_observed
printf '%s\n' "$trace_text" | rg -q '"kind":"send".*"type":"client.hello"' \
  || blocked client_hello_not_observed
python3 - "$before_text" "$after_text" <<'PY'
import re
import sys

before = open(sys.argv[1], encoding="utf-8").read()
after = open(sys.argv[2], encoding="utf-8").read()
pattern = re.compile(r"Room sequence\s+(\d+)")
before_match = pattern.search(before)
after_match = pattern.search(after)
if before_match is None or after_match is None or before_match.group(1) == after_match.group(1):
    raise SystemExit("projection_room_sequence_did_not_change")
print(f"room_seq_before={before_match.group(1)} room_seq_after={after_match.group(1)}")
PY
printf '%s\n' "$page_html" | rg -q 'Action accepted at the canonical Room Head\.' \
  || blocked authorized_action_not_visible
printf '%s\n' "$(cmux browser "$surface" eval --script 'String(window.__WORLDSTREAM_LIVE_SESSION__)')" | rg -q 'undefined' \
  || blocked ephemeral_config_not_cleared
printf '%s\n' "$url_text" >"$root/browser-url.txt"
printf '%s\n' "$page_html" >"$root/browser-page.html"
printf '%s\n' "$trace_text" >"$root/browser-trace.json"
printf '%s\n' "$console_text" >"$root/browser-console.txt"
printf '%s\n' "$errors_text" >"$root/browser-errors.txt"
printf '%s\n' "$local_storage" >"$root/browser-local-storage.txt"
printf '%s\n' "$session_storage" >"$root/browser-session-storage.txt"

# A reload proves the normal live path can issue another one-time ticket and
# reattach. The current console intentionally starts at a full reset because
# it does not persist a cursor in browser storage.
cmux browser "$surface" reload >"$root/browser-reload.txt" || blocked browser_reload_failed
cmux browser "$surface" wait --text "Live session · live" --timeout-ms 30000 \
  >"$root/browser-reconnect.txt" || blocked browser_reconnect_failed
reconnect_trace="$(cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])')"
printf '%s\n' "$reconnect_trace" | rg -q '"kind":"fetch".*v1/stream/ticket.*"has_authorization":true' \
  || blocked reconnect_ticket_not_observed
printf '%s\n' "$(cmux browser "$surface" eval --script 'String(window.__WORLDSTREAM_LIVE_SESSION__)')" \
  | rg -q 'undefined' || blocked ephemeral_config_not_cleared_after_reload
printf '%s\n' "$reconnect_trace" >"$root/browser-reconnect-trace.json"

printf 'status=passed\n'
printf 'surface=%s\n' "$surface"
printf 'base_url=%s\n' "$base_url"
printf 'console_origin=http://127.0.0.1:%s\n' "$console_port"
printf 'evidence=real_cmux_browser_production_console\n'
printf 'ticket_first=verified_by_native_transport_and_network_path\n'
printf 'hello_attach_sync_action_projection_update=reached\n'
printf 'reload_reconnect=verified\n'
printf 'cursor_resume=not_claimed_full_reset_is_explicit\n'
