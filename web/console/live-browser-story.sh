#!/usr/bin/env bash
set -euo pipefail
workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$workspace_dir"
blocked() { printf 'blocked:%s\n' "$1" >&2; exit 2; }
python_bin="${WORLDSTREAM_PYTHON:-$workspace_dir/sdk/python/.venv/bin/python}"
[[ -x "$python_bin" ]] || blocked python_sdk_runtime_missing
browser_mode="${WORLDSTREAM_BROWSER_MODE:-cmux}"
package_mode="${WORLDSTREAM_BROWSER_PACKAGE_MODE:-0}"
prebuilt_ui="${WORLDSTREAM_BROWSER_PREBUILT_UI:-0}"
report_path="${WORLDSTREAM_BROWSER_REPORT:-}"
worldstreamd="${WORLDSTREAM_BROWSER_WORLDSTREAMD:-$workspace_dir/target/debug/worldstreamd}"
ui_dir="${WORLDSTREAM_BROWSER_UI_DIR:-$workspace_dir/web/console/dist}"
sdk_src="${WORLDSTREAM_BROWSER_SDK_SRC:-$workspace_dir/sdk/python/src}"
heist_dir="${WORLDSTREAM_BROWSER_HEIST_DIR:-$workspace_dir/examples/heist}"
heist_live_dir="$heist_dir/wave10_live"
[[ -x "$worldstreamd" ]] || blocked worldstreamd_binary_missing
[[ -f "$ui_dir/index.html" ]] || blocked production_console_build_missing
[[ -f "$sdk_src/worldstream_sdk/__init__.py" ]] || blocked python_sdk_source_missing
for required_client in seed_browser_room.py run_browser_story.py run_absent_broker_live.py browser_trace_init.js; do
  [[ -f "$heist_live_dir/$required_client" ]] || blocked packaged_heist_client_missing
done
if [[ "$package_mode" = 1 ]]; then
  [[ "$browser_mode" = cdp ]] || blocked package_browser_must_use_pinned_cdp
  [[ -n "$report_path" ]] || blocked package_browser_report_required
  package_root="${WORLDSTREAM_BROWSER_PACKAGE_ROOT:-}"
  [[ -n "$package_root" && -d "$package_root" ]] || blocked package_root_missing
  if ! "$python_bin" - "$package_root" "$worldstreamd" "$ui_dir" "$sdk_src" "$heist_dir" <<'PY'
import stat
import sys
from pathlib import Path

root = Path(sys.argv[1])
if root.is_symlink() or not root.is_dir():
    raise SystemExit(1)
root = root.resolve(strict=True)
expected = (
    (Path(sys.argv[2]), root / "bin/worldstreamd", "file"),
    (Path(sys.argv[3]), root / "ui", "directory"),
    (Path(sys.argv[4]), root / "sdk/python/src", "directory"),
    (Path(sys.argv[5]), root / "examples/heist", "directory"),
)
for supplied, required, kind in expected:
    if supplied != required or supplied.resolve(strict=True) != required:
        raise SystemExit(1)
    relative = required.relative_to(root)
    current = root
    for part in relative.parts:
        current /= part
        mode = current.lstat().st_mode
        if stat.S_ISLNK(mode):
            raise SystemExit(1)
    mode = required.lstat().st_mode
    if (kind == "file" and not stat.S_ISREG(mode)) or (
        kind == "directory" and not stat.S_ISDIR(mode)
    ):
        raise SystemExit(1)
PY
  then
    blocked nonpackage_runtime_path_rejected
  fi
else
  [[ "$package_mode" = 0 ]] || blocked package_mode_invalid
  command -v npm >/dev/null 2>&1 || blocked npm_unavailable_for_production_build
fi
if [[ "$browser_mode" = cdp ]]; then
  cdp_adapter="${WORLDSTREAM_CDP_ADAPTER:-$workspace_dir/scripts/cdp-browser.py}"
  [[ -f "$cdp_adapter" ]] || blocked browser_cli_unavailable
  [[ -n "${WORLDSTREAM_CDP_STATE_DIR:-}" ]] || blocked browser_state_directory_missing
  cmux() { "$python_bin" "$cdp_adapter" "$@"; }
  workspace_ref="pinned-cdp-browser"
  window_ref="pinned-cdp-browser"
elif [[ "$browser_mode" = cmux ]]; then
  command -v cmux >/dev/null 2>&1 || blocked browser_cli_unavailable
  workspace_ref="${CMUX_WORKSPACE_ID:-}"
  [[ -n "$workspace_ref" ]] || blocked cmux_workspace_context_missing
  window_ref="${CMUX_WINDOW_ID:-$(cmux identify --json | "$python_bin" -c 'import json,sys; print(json.load(sys.stdin)["caller"]["window_ref"])')}"
else
  blocked browser_mode_invalid
fi
root="$(mktemp -d -t worldstream-live-browser.XXXXXX)"
chmod 700 "$root"
server_pid=""; console_pid=""; driver_pid=""; surface=""; claim=""
private_canary="required_tool_thermal_key"
preserve_artifacts="${KEEP_LIVE_BROWSER_ARTIFACTS:-0}"
story_timeout="${WORLDSTREAM_BROWSER_STORY_TIMEOUT:-240}"
[[ "$story_timeout" =~ ^[1-9][0-9]*$ ]] || blocked browser_story_timeout_invalid
story_started_ns="$($python_bin -c 'import time; print(time.monotonic_ns())')"
cleanup() {
  [[ -z "$surface" ]] || cmux close-surface --surface "$surface" --workspace "$workspace_ref" --window "$window_ref" >/dev/null 2>&1 || true
  for pid in "$driver_pid" "$console_pid" "$server_pid"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then kill "$pid" >/dev/null 2>&1 || true; wait "$pid" 2>/dev/null || true; fi
  done
  if [[ "$preserve_artifacts" = 1 ]]; then
    rm -f "$root/authority.secret" "$root/operator.bearer" "$root/browser-manifest.json" "$root/driver-manifest.json"
    rm -rf "$root/data"
    printf 'evidence_root=%s\n' "$root" >&2
  else
    rm -rf "$root"
  fi
}
trap cleanup EXIT
data_dir="$root/data"; mkdir -m 700 "$data_dir"
if [[ "$package_mode" = 0 && "$prebuilt_ui" != 1 ]]; then
  if ! npm --prefix web/console run build >"$root/console-build.log" 2>&1; then
    tail -80 "$root/console-build.log" >&2 || true
    blocked production_console_build_failed
  fi
fi
bootstrap_secret="$root/authority.secret"; operator_bearer="$root/operator.bearer"
manifest="$root/browser-manifest.json"; driver_manifest="$root/driver-manifest.json"
"$python_bin" -c 'import os,secrets,sys; secret_path,bearer_path=sys.argv[1:]; secret=secrets.token_bytes(32); fd=os.open(secret_path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600); handle=os.fdopen(fd,"wb"); handle.write(secret); handle.flush(); os.fsync(handle.fileno()); handle.close(); open(bearer_path,"x",encoding="utf-8").write("wsb1:"+secret.hex()+"\n"); os.chmod(bearer_path,0o600)' "$bootstrap_secret" "$operator_bearer"
port="$("$python_bin" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
base_url="http://127.0.0.1:$port"
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" RUST_LOG="${WORLDSTREAM_LIVE_RUST_LOG:-warn}" "$worldstreamd" --data-dir "$data_dir" --bind "127.0.0.1:$port" >"$root/worldstreamd.log" 2>&1 &
server_pid="$!"
ready=0
for _ in $(seq 1 150); do
  if curl -fsS "$base_url/readyz" >/dev/null 2>&1; then ready=1; break; fi
  kill -0 "$server_pid" >/dev/null 2>&1 || blocked daemon_exited_before_ready
  sleep 0.1
done
[[ "$ready" = 1 ]] || blocked daemon_ready_timeout
if ! PYTHONPATH="$sdk_src:$heist_live_dir" "$python_bin" "$heist_live_dir/seed_browser_room.py" --base-url "$base_url" --operator-bearer-file "$operator_bearer" --output "$manifest" --driver-output "$driver_manifest" >"$root/seed-summary.txt" 2>"$root/seed-error.txt"; then
  tail -40 "$root/seed-error.txt" >&2 || true
  blocked seed_browser_room_failed
fi
console_port="$("$python_bin" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
"$python_bin" -m http.server "$console_port" --bind 127.0.0.1 --directory "$ui_dir" >"$root/console-http.log" 2>&1 &
console_pid="$!"
console_url="http://127.0.0.1:$console_port/?view=participant"
browser_open="$root/browser-open.json"
cmux --json browser open about:blank --workspace "$workspace_ref" --window "$window_ref" --focus false >"$browser_open" || blocked browser_surface_create_failed
surface="$("$python_bin" -c 'import json,sys; value=json.load(open(sys.argv[1])); ref=value.get("surface_ref"); print(ref if isinstance(ref,str) else "")' "$browser_open")"
[[ "$surface" == surface:* ]] || blocked browser_surface_ref_missing
bootstrap_json="$("$python_bin" -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])),separators=(",",":")))' "$manifest")"
init_script="window.__WORLDSTREAM_LIVE_SESSION__=$bootstrap_json;$(tr -d '\n' < "$heist_live_dir/browser_trace_init.js")"
cmux browser "$surface" addinitscript --script "$init_script" >"$root/browser-init.txt" || blocked browser_init_script_failed
cmux browser "$surface" navigate "$console_url" >"$root/browser-navigate.txt" || blocked browser_navigation_failed
cmux browser "$surface" wait --load-state complete --timeout-ms 15000 >"$root/browser-load.txt" || blocked browser_page_load_timeout
if ! cmux browser "$surface" wait --text "Live session · live" --timeout-ms 30000 >"$root/browser-live.txt"; then
  cmux browser "$surface" get text body >"$root/browser-live-body.txt" 2>/dev/null || true
  cmux browser "$surface" console list >"$root/browser-live-diagnostics.txt" 2>/dev/null || true
  cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])' >"$root/browser-live-trace.txt" 2>/dev/null || true
  tail -80 "$root/browser-live-body.txt" >&2 || true
  tail -80 "$root/browser-live-diagnostics.txt" >&2 || true
  tail -80 "$root/browser-live-trace.txt" >&2 || true
  blocked live_session_did_not_reach_live
fi
printf '%s\n' "$(cmux browser "$surface" eval --script 'String(window.__WORLDSTREAM_LIVE_SESSION__)')" | rg -q undefined || blocked ephemeral_config_not_cleared
signal() { : >"$root/$1"; }
wait_signal() {
  for _ in $(seq 1 "$((story_timeout * 10))"); do
    [[ -f "$root/$1" ]] && return 0
    driver_state="$(ps -o stat= -p "$driver_pid" 2>/dev/null || true)"
    if [[ -n "$driver_pid" ]] && { ! kill -0 "$driver_pid" >/dev/null 2>&1 || [[ "$driver_state" == *Z* ]]; }; then
      printf 'driver_exited_before_signal=%s\n' "$1" >&2
      tail -80 "$root/driver.log" >&2 || true
      driver_status=0
      wait "$driver_pid" || driver_status=$?
      printf 'driver_exit_status=%s\n' "$driver_status" >&2
      blocked "browser_driver_failed_before_$1"
    fi
    sleep 0.1
  done
  printf 'driver_log_tail=%s\n' "$1" >&2
  tail -80 "$root/driver.log" >&2 || true
  blocked "browser_story_timeout_$1"
}
wait_phase() {
  local expected="$1"
  local evidence_file="$2"
  local minimum_sequence="$3"
  local failure_reason="$4"
  for _ in $(seq 1 300); do
    phase_probe="$(cmux browser "$surface" eval --script 'JSON.stringify((() => { const phase = document.querySelector(".phase-card strong")?.textContent?.trim() || ""; const sequence = [...document.querySelectorAll(".phase-card .sequence")].find((node) => node.textContent?.includes("Room sequence"))?.textContent || ""; const match = sequence.match(/[0-9]+/); return { phase, roomSequence: match ? Number(match[0]) : 0 }; })())' 2>/dev/null || true)"
    if "$python_bin" -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("phase") == sys.argv[1] and value.get("roomSequence", 0) >= int(sys.argv[2]) else 1)' "$expected" "$minimum_sequence" <<<"$phase_probe"; then
      printf '%s\n' "$phase_probe" >"$root/$evidence_file"
      return 0
    fi
    driver_state="$(ps -o stat= -p "$driver_pid" 2>/dev/null || true)"
    if [[ -n "$driver_pid" ]] && { ! kill -0 "$driver_pid" >/dev/null 2>&1 || [[ "$driver_state" == *Z* ]]; }; then
      printf 'driver_exited_before_phase=%s\n' "$expected" >&2
      tail -80 "$root/driver.log" >&2 || true
      driver_status=0
      wait "$driver_pid" || driver_status=$?
      printf 'driver_exit_status=%s\n' "$driver_status" >&2
      blocked "browser_driver_failed_before_$failure_reason"
    fi
    sleep 0.1
  done
  phase_body="$(cmux browser "$surface" get text body 2>/dev/null || true)"
  if [[ -n "$claim" ]]; then phase_body="${phase_body//$claim/[authorized participant claim redacted]}"; fi
  printf '%s\n' "$phase_body" >"$root/${evidence_file%.txt}-body.txt"
  cmux browser "$surface" eval --script 'JSON.stringify((window.__WORLDSTREAM_BROWSER_TRACE__ || []).map((entry) => ({kind: entry.kind, type: entry.type, status: entry.status, url: entry.url, frame_seq: entry.frame_seq, cause_room_seq: entry.cause_room_seq, phase: entry.phase, offer_types: entry.offer_types})))' >"$root/${evidence_file%.txt}-trace.txt" 2>/dev/null || true
  blocked "$failure_reason"
}
wait_action_form() {
  local action_type="$1"
  local failure_reason="$2"
  for _ in $(seq 1 300); do
    if cmux browser "$surface" wait --selector "form[data-action-type=\"$action_type\"]" --timeout-ms 500 >/dev/null 2>&1; then
      return 0
    fi
    form_probe="$(cmux browser "$surface" eval --script "JSON.stringify(Array.from(document.querySelectorAll('form')).some((form) => form.dataset.actionType === '$action_type'))" 2>/dev/null || true)"
    [[ "$form_probe" = true || "$form_probe" = '"true"' ]] && return 0
    driver_state="$(ps -o stat= -p "$driver_pid" 2>/dev/null || true)"
    if [[ -n "$driver_pid" ]] && { ! kill -0 "$driver_pid" >/dev/null 2>&1 || [[ "$driver_state" == *Z* ]]; }; then
      printf 'driver_exited_before_form=%s\n' "$action_type" >&2
      tail -80 "$root/driver.log" >&2 || true
      driver_status=0
      wait "$driver_pid" || driver_status=$?
      printf 'driver_exit_status=%s\n' "$driver_status" >&2
      blocked "browser_driver_failed_before_$failure_reason"
    fi
    sleep 0.1
  done
  phase_body="$(cmux browser "$surface" get text body 2>/dev/null || true)"
  if [[ -n "$claim" ]]; then phase_body="${phase_body//$claim/[authorized participant claim redacted]}"; fi
  printf '%s\n' "$phase_body" >"$root/${action_type}-form-body.txt"
  cmux browser "$surface" eval --script 'JSON.stringify((window.__WORLDSTREAM_BROWSER_TRACE__ || []).map((entry) => ({kind: entry.kind, type: entry.type, status: entry.status, url: entry.url, frame_seq: entry.frame_seq, cause_room_seq: entry.cause_room_seq, phase: entry.phase, offer_types: entry.offer_types})))' >"$root/${action_type}-form-trace.txt" 2>/dev/null || true
  blocked "$failure_reason"
}
wait_action_submit_enabled() {
  local action_type="$1"
  local failure_reason="$2"
  for _ in $(seq 1 300); do
    submit_probe="$(cmux browser "$surface" eval --script "JSON.stringify(Boolean((() => { const button = document.querySelector('form[data-action-type=\"$action_type\"] button[type=\"submit\"]'); return button && !button.disabled && button.getAttribute('aria-disabled') !== 'true'; })()))" 2>/dev/null || true)"
    [[ "$submit_probe" = true || "$submit_probe" = '"true"' ]] && return 0
    driver_state="$(ps -o stat= -p "$driver_pid" 2>/dev/null || true)"
    if [[ -n "$driver_pid" ]] && { ! kill -0 "$driver_pid" >/dev/null 2>&1 || [[ "$driver_state" == *Z* ]]; }; then
      printf 'driver_exited_before_submit=%s\n' "$action_type" >&2
      tail -80 "$root/driver.log" >&2 || true
      driver_status=0
      wait "$driver_pid" || driver_status=$?
      printf 'driver_exit_status=%s\n' "$driver_status" >&2
      blocked "browser_driver_failed_before_$failure_reason"
    fi
    sleep 0.1
  done
  phase_body="$(cmux browser "$surface" get text body 2>/dev/null || true)"
  if [[ -n "$claim" ]]; then phase_body="${phase_body//$claim/[authorized participant claim redacted]}"; fi
  printf '%s\n' "$phase_body" >"$root/${action_type}-submit-body.txt"
  cmux browser "$surface" eval --script "JSON.stringify(Array.from(document.querySelectorAll('form[data-action-type=\"$action_type\"] button')).map((button) => ({disabled:button.disabled,ariaDisabled:button.getAttribute('aria-disabled')})))" >"$root/${action_type}-submit-probe.txt" 2>/dev/null || true
  cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])' >"$root/${action_type}-submit-trace.txt" 2>/dev/null || true
  blocked "$failure_reason"
}
browser_action_send_count() {
  cmux browser "$surface" eval --script '(window.__WORLDSTREAM_BROWSER_TRACE__ || []).filter((entry) => entry.kind === "send" && entry.type === "action.submit").length'
}
wait_new_action_send() {
  local previous_count="$1"
  local evidence_file="$2"
  local failure_reason="$3"
  [[ "$previous_count" =~ ^[0-9]+$ ]] || blocked browser_action_trace_invalid
  for _ in $(seq 1 300); do
    current_count="$(browser_action_send_count 2>/dev/null || true)"
    if [[ "$current_count" =~ ^[0-9]+$ ]] && (( current_count > previous_count )); then
      printf '%s\n' "$current_count" >"$root/$evidence_file"
      return 0
    fi
    sleep 0.1
  done
  cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])' >"$root/${evidence_file%.txt}-trace.txt" 2>/dev/null || true
  blocked "$failure_reason"
}
assert_claim_absent() {
  local label="$1"
  local value="$2"
  [[ "$value" != *"$claim"* ]] || blocked "private_claim_leaked_$label"
  printf '%s\n' "$value" >"$root/$label.txt"
}
wait_phase Briefing briefing-phase.txt 0 briefing_phase_not_visible
signal browser-ready; signal stale-ready
cmux browser "$surface" eval --script 'window.__WORLDSTREAM_HOLD_OBSERVATIONS__=true' >/dev/null
printf 'driver_launching timeout=%s\n' "$story_timeout" >"$root/driver.log"
PYTHONUNBUFFERED=1 PYTHONPATH="$sdk_src:$heist_live_dir" "$python_bin" -u "$heist_live_dir/run_browser_story.py" \
  --driver-manifest "$driver_manifest" --operator-bearer-file "$operator_bearer" \
  --browser-ready "$root/browser-ready" --stale-ready "$root/stale-ready" \
  --stale-server-advanced "$root/stale-server-advanced" --navigator-inspected "$root/navigator-inspected" \
  --briefing-open "$root/briefing-open" --navigator-published "$root/navigator-published" \
  --negotiation-open "$root/negotiation-open" --plan-proposed "$root/plan-proposed" \
  --plan-file "$root/plan-id" --commitment-open "$root/commitment-open" \
  --navigator-committed "$root/navigator-committed" --result-ready "$root/result-ready" \
  --navigator-acked "$root/navigator-acked" --result-file "$root/story-result.json" \
  --timeout "$story_timeout" --timer-timeout "$story_timeout" --offer-timeout 20 >>"$root/driver.log" 2>&1 &
driver_pid="$!"
wait_signal stale-server-advanced
cmux browser "$surface" fill 'form[data-action-type="inspect_clue"] input[name="clue_id"]' route >"$root/stale-fill.txt" || blocked inspect_clue_input_missing
cmux browser "$surface" click 'form[data-action-type="inspect_clue"] button[type="submit"]' >"$root/stale-click.txt" || blocked stale_action_submit_missing
cmux browser "$surface" wait --text "stale Head" --timeout-ms 30000 >"$root/stale-rejected.txt" || blocked stale_action_not_rejected
cmux browser "$surface" get text body >"$root/stale-body.txt" || blocked stale_dom_snapshot_missing
rg -q 'stale Head' "$root/stale-body.txt" || blocked stale_dom_snapshot_invalid
cmux browser "$surface" eval --script 'window.__WORLDSTREAM_HOLD_OBSERVATIONS__=false;window.__WORLDSTREAM_OBSERVATION_QUEUE__=[]' >/dev/null
cmux browser "$surface" click '[data-testid="resync-session"]' >"$root/resync-click.txt" || blocked resync_control_missing
# The authorized claim is captured only after the resynchronized inspect action.
# Keep the diagnostic substitution safe if this pre-claim wait fails under set -u.
claim=""
resync_live=0
for _ in $(seq 1 300); do
  resync_probe="$(cmux browser "$surface" eval --script 'JSON.stringify({status:document.querySelector(".view-note strong")?.textContent || "", sockets:(window.__WORLDSTREAM_BROWSER_TRACE__ || []).filter((entry) => entry.kind === "websocket").length, syncAcks:(window.__WORLDSTREAM_BROWSER_TRACE__ || []).filter((entry) => entry.kind === "receive" && entry.type === "room.sync_acked").length})' 2>/dev/null || true)"
  if "$python_bin" -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("status") == "Live session · live" and value.get("sockets", 0) >= 2 and value.get("syncAcks", 0) >= 2 else 1)' <<<"$resync_probe"; then
    printf '%s\n' "$resync_probe" >"$root/resync-live.txt"
    resync_live=1
    break
  fi
  driver_state="$(ps -o stat= -p "$driver_pid" 2>/dev/null || true)"
  if [[ -n "$driver_pid" ]] && { ! kill -0 "$driver_pid" >/dev/null 2>&1 || [[ "$driver_state" == *Z* ]]; }; then
    printf 'driver_exited_before_signal=resync-live\n' >&2
    tail -80 "$root/driver.log" >&2 || true
    driver_status=0
    wait "$driver_pid" || driver_status=$?
    printf 'driver_exit_status=%s\n' "$driver_status" >&2
    blocked browser_driver_failed_before_resync_live
  fi
  sleep 0.1
done
if [[ "$resync_live" != 1 ]]; then
  resync_body="$(cmux browser "$surface" get text body 2>/dev/null || true)"
  if [[ -n "$claim" ]]; then resync_body="${resync_body//$claim/[authorized participant claim redacted]}"; fi
  printf '%s\n' "$resync_body" >"$root/resync-body.txt"
  cmux browser "$surface" eval --script 'JSON.stringify((window.__WORLDSTREAM_BROWSER_TRACE__ || []).map((entry) => ({kind: entry.kind, type: entry.type, status: entry.status, url: entry.url})))' >"$root/resync-trace.txt" 2>/dev/null || true
  blocked resync_did_not_reach_live
fi
cmux browser "$surface" fill 'form[data-action-type="inspect_clue"] input[name="clue_id"]' route >"$root/inspect-fill.txt" || blocked resynced_inspect_input_missing
cmux browser "$surface" click 'form[data-action-type="inspect_clue"] button[type="submit"]' >"$root/inspect-click.txt" || blocked resynced_inspect_submit_missing
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/inspect-accepted.txt" || blocked resynced_inspect_not_accepted
claim="$(cmux browser "$surface" eval --script 'document.querySelector("[data-private-claim]")?.textContent || ""')"
[[ -n "$claim" ]] || blocked participant_private_claim_not_visible_after_inspect
signal navigator-inspected
wait_signal briefing-open
[[ -n "$claim" && "$claim" != *"navigator-private-clue"* ]] || blocked participant_private_claim_not_visible
wait_action_form publish_clue publish_clue_form_not_ready
cmux browser "$surface" fill 'form[data-action-type="publish_clue"] input[name="clue_id"]' route >"$root/publish-clue-fill.txt" || blocked publish_clue_form_missing
cmux browser "$surface" fill 'form[data-action-type="publish_clue"] input[name="claim_code"]' "$claim" >"$root/publish-claim-fill.txt" || blocked publish_claim_input_missing
publish_send_count="$(browser_action_send_count)"
cmux browser "$surface" click 'form[data-action-type="publish_clue"] button[type="submit"]' >"$root/publish-click.txt" || blocked publish_action_submit_missing
wait_new_action_send "$publish_send_count" publish-action-send-count.txt publish_action_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/publish-accepted.txt" || blocked publish_action_not_accepted
signal navigator-published
wait_signal negotiation-open
wait_phase Negotiation negotiation-phase.txt 3 negotiation_phase_not_visible
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="route"]' service >"$root/plan-route.txt" || blocked propose_plan_form_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="entry_window"]' early >"$root/plan-window.txt" || blocked propose_plan_window_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="required_tool"]' thermal_key >"$root/plan-tool.txt" || blocked propose_plan_tool_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="extraction"]' boat >"$root/plan-extraction.txt" || blocked propose_plan_extraction_missing
plan_send_count="$(browser_action_send_count)"
cmux browser "$surface" click 'form[data-action-type="propose_plan"] button[type="submit"]' >"$root/plan-click.txt" || blocked propose_plan_submit_missing
wait_new_action_send "$plan_send_count" plan-action-send-count.txt propose_plan_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/plan-accepted.txt" || blocked propose_plan_not_accepted
signal plan-proposed
wait_signal commitment-open
wait_phase Commitment commitment-phase.txt 6 commitment_phase_not_visible
wait_action_form commit_move commit_form_not_ready
wait_action_submit_enabled commit_move commit_submit_not_ready
plan_id="$(for _ in $(seq 1 120); do [[ -s "$root/plan-id" ]] && break; sleep 0.1; done; tr -d '\n' <"$root/plan-id")"
[[ -n "$plan_id" ]] || blocked public_plan_id_not_available
cmux browser "$surface" fill 'form[data-action-type="commit_move"] input[name="selected_plan_id"]' "$plan_id" >"$root/nav-commit-plan.txt" || blocked commit_form_missing
commit_send_count="$(browser_action_send_count)"
cmux browser "$surface" click 'form[data-action-type="commit_move"] button[type="submit"]' >"$root/nav-commit-click.txt" || blocked commit_submit_missing
wait_new_action_send "$commit_send_count" commit-action-send-count.txt navigator_commit_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/nav-commit-accepted.txt" || blocked navigator_commit_not_accepted
signal navigator-committed
wait_signal result-ready
wait_phase Result result-phase.txt 3 result_phase_not_visible
cmux browser "$surface" click '#replay-tab' >"$root/precomplete-replay-tab.txt" || blocked replay_tab_missing_precomplete
cmux browser "$surface" wait --text "Replay · read-only" --timeout-ms 15000 >"$root/precomplete-replay.txt" || blocked precomplete_replay_missing
precomplete="$(cmux browser "$surface" get text body)"
printf '%s\n' "$precomplete" | rg -q 'Locked until the Activity Phase is Complete' || blocked final_reveal_unlocked_precomplete
printf '%s\n' "$precomplete" | rg -q 'Historical authorization' || blocked historical_authorization_missing
printf '%s\n' "${precomplete//$claim/[deliberately published claim redacted]}" >"$root/precomplete-body.txt"
cmux browser "$surface" click '#participant-tab' >/dev/null || blocked participant_tab_missing_for_ack
cmux browser "$surface" wait --text "acknowledge_result" --timeout-ms 30000 >/dev/null || blocked acknowledge_offer_missing
ack_send_count="$(browser_action_send_count)"
cmux browser "$surface" click 'form[data-action-type="acknowledge_result"] button[type="submit"]' >"$root/nav-ack-click.txt" || blocked acknowledge_submit_missing
wait_new_action_send "$ack_send_count" acknowledge-action-send-count.txt acknowledge_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/nav-ack-accepted.txt" || blocked acknowledge_not_accepted
signal navigator-acked
wait_signal story-result.json
wait "$driver_pid" || blocked full_browser_story_driver_failed
wait_phase Complete complete-phase.txt 3 complete_phase_not_visible
result="$(<"$root/story-result.json")"
printf '%s\n' "$result" | rg -q '"status":"completed"' || blocked full_browser_story_not_completed
for tab in public participant operator replay; do
  cmux browser "$surface" click "#$tab-tab" >"$root/$tab-tab.txt" || blocked "${tab}_tab_missing"
  body="$(cmux browser "$surface" get text body)"
  [[ "$body" != *"$private_canary"* ]] || blocked "private_canary_leaked_$tab"
  if [[ "$tab" = public || "$tab" = participant ]]; then
    [[ "$body" == *"$claim"* ]] || blocked deliberately_published_claim_not_visible_after_story
  fi
  body_evidence="${body//$claim/[deliberately published claim redacted]}"
  printf '%s\n' "$body_evidence" >"$root/$tab-body.txt"
  if printf '%s\n' "$body" | rg -qi 'navigator-private-clue|sealed-value|private_clue|own_commitment|chain-of-thought|Bearer[[:space:]]|wsb1:[0-9a-f]{64}|wst1:[0-9a-f]{64}|raw (core|activity) state'; then blocked "privacy_negative_$tab"; fi
done
public_body="$(<"$root/public-body.txt")"
printf '%s\n' "$public_body" | rg -q 'Published clues|Plans and public review|Aggregate result|commitments remain withheld' || blocked public_story_surface_incomplete
printf '%s\n' "$public_body" | rg -q 'endorsements|challenges' || blocked public_review_counts_missing
operator_body="$(<"$root/operator-body.txt")"
printf '%s\n' "$operator_body" | rg -q 'Membership|Session & frame|Runner & Activation|Timer|Room integrity' || blocked operator_diagnostics_incomplete
replay_body="$(<"$root/replay-body.txt")"
printf '%s\n' "$replay_body" | rg -q 'Replay · read-only|Verified|Core hash|Activity hash|Aggregate hash|Transition hash|Available' || blocked replay_or_final_reveal_incomplete
if rg -n -P 'wsb1:[0-9a-f]{64}|wst1:[0-9a-f]{64}' "$ui_dir" >/dev/null 2>&1; then blocked credential_or_ticket_in_production_assets; fi
trace_text="$(cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])')"
browser_diagnostics_raw="$(cmux browser "$surface" console list 2>/dev/null)" || blocked browser_diagnostics_unavailable
browser_diagnostics="$("$python_bin" -c '
import json
import sys

def pairs(items):
    value = {}
    for key, item in items:
        if key in value:
            raise ValueError("duplicate diagnostic key")
        value[key] = item
    return value

value = json.loads(
    sys.stdin.read(),
    object_pairs_hook=pairs,
    parse_constant=lambda item: (_ for _ in ()).throw(ValueError(item)),
)
schemas = {
    "console.warn": {"kind", "message"},
    "console.error": {"kind", "message"},
    "error": {"kind", "message", "source", "line", "column", "stack"},
    "rejection": {"kind", "message", "stack"},
}
if not isinstance(value, list) or len(value) > 100:
    raise SystemExit(1)
for item in value:
    if not isinstance(item, dict) or set(item) != schemas.get(item.get("kind")):
        raise SystemExit(1)
    for key, maximum in (("message", 1024), ("source", 1024), ("stack", 4096)):
        if key in item and (not isinstance(item[key], str) or len(item[key]) > maximum):
            raise SystemExit(1)
    for key in ("line", "column"):
        if key in item and (type(item[key]) is not int or not 0 <= item[key] <= 2**31 - 1):
            raise SystemExit(1)
print(json.dumps(value, sort_keys=True, separators=(",", ":")))
' <<<"$browser_diagnostics_raw")" || blocked browser_diagnostics_invalid
# Warnings are release-blocking too: this acceptance has no warning allowlist,
# so a new browser warning cannot silently enter signed release evidence.
printf '%s\n' "$browser_diagnostics" >"$root/browser-diagnostics.txt"
[[ "$browser_diagnostics" = '[]' ]] || blocked browser_diagnostics_not_clean
url_text="$(cmux browser "$surface" get url)"
local_storage="$(cmux browser "$surface" storage local get 2>/dev/null || true)"
session_storage="$(cmux browser "$surface" storage session get 2>/dev/null || true)"
assert_claim_absent browser-trace "$trace_text"
assert_claim_absent browser-diagnostics "$browser_diagnostics"
assert_claim_absent browser-url "$url_text"
assert_claim_absent browser-local-storage "$local_storage"
assert_claim_absent browser-session-storage "$session_storage"
printf '%s\n' "$trace_text" "$browser_diagnostics" "$url_text" "$local_storage" "$session_storage" | rg -Fq -- "$private_canary" && blocked private_canary_in_browser_diagnostics || true
printf '%s\n' "$trace_text" "$browser_diagnostics" "$url_text" "$local_storage" "$session_storage" | rg -qi 'wsb1:[0-9a-f]{64}|wst1:[0-9a-f]{64}' && blocked browser_secret_or_ticket_leaked || true
printf '%s\n' "$trace_text" | rg -q '"kind":"fetch".*v1/stream/ticket.*"has_authorization":true' || blocked ticket_request_not_observed
printf '%s\n' "$trace_text" | rg -q '"kind":"send".*"ticket_first":true' || blocked ticket_first_frame_not_observed
printf '%s\n' "$trace_text" | rg -q '"kind":"send".*"type":"client.hello"' || blocked client_hello_not_observed
rg -F -- "$claim" "$root" >/dev/null 2>&1 && blocked private_claim_in_evidence || true
if [[ -n "$report_path" ]]; then
  [[ "$browser_mode" = cdp ]] || blocked structured_report_requires_pinned_cdp
  [[ ! -L "$report_path" && ! -d "$report_path" ]] || blocked browser_report_path_unsafe
  mkdir -p "$(dirname "$report_path")"
  story_finished_ns="$($python_bin -c 'import time; print(time.monotonic_ns())')"
  "$python_bin" - \
    "$report_path" "$browser_open" "$root/story-result.json" "$root" \
    "$worldstreamd" "$ui_dir" "$sdk_src" "$heist_dir" "$cdp_adapter" \
    "$package_mode" "$workspace_dir" "$story_started_ns" "$story_finished_ns" <<'PY'
import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path


def reject_pairs(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON key")
        value[key] = item
    return value


def read_json(path):
    value = json.loads(
        Path(path).read_text(encoding="utf-8"),
        object_pairs_hook=reject_pairs,
        parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)),
    )
    if not isinstance(value, dict):
        raise TypeError("JSON object required")
    return value


def digest_file(path):
    target = Path(path)
    mode = target.lstat().st_mode
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise ValueError("regular file required")
    digest = hashlib.sha256()
    size = 0
    with target.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return {"sha256": "sha256:" + digest.hexdigest(), "size_bytes": size}


def tree_identity(path, required):
    root = Path(path)
    if root.is_symlink() or not root.is_dir():
        raise ValueError("regular tree required")
    entries = []
    for candidate in sorted(root.rglob("*")):
        mode = candidate.lstat().st_mode
        if stat.S_ISLNK(mode) or (not stat.S_ISREG(mode) and not stat.S_ISDIR(mode)):
            raise ValueError("unsafe tree entry")
        if stat.S_ISREG(mode):
            record = digest_file(candidate)
            entries.append(
                {
                    "path": candidate.relative_to(root).as_posix(),
                    "sha256": record["sha256"],
                    "size_bytes": record["size_bytes"],
                }
            )
    paths = {entry["path"] for entry in entries}
    if not entries or not set(required).issubset(paths):
        raise ValueError("tree inventory incomplete")
    canonical = (
        json.dumps(entries, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()
    return {
        "tree_sha256": "sha256:" + hashlib.sha256(canonical).hexdigest(),
        "file_count": len(entries),
        "total_bytes": sum(entry["size_bytes"] for entry in entries),
    }


def evidence_digest(root, name):
    record = digest_file(root / name)
    if record["size_bytes"] <= 0:
        raise ValueError("empty DOM evidence")
    return record["sha256"]


(
    destination,
    browser_open_path,
    story_path,
    evidence_root,
    daemon_path,
    ui_path,
    sdk_path,
    heist_path,
    adapter_path,
    package_mode,
    workspace_path,
    started_ns,
    finished_ns,
) = sys.argv[1:]
evidence_root = Path(evidence_root)
browser_open = read_json(browser_open_path)
browser = browser_open.get("browser")
if (
    not isinstance(browser, dict)
    or set(browser)
    != {
        "product",
        "version",
        "sha256",
        "size_bytes",
        "version_output",
        "distribution",
    }
    or browser.get("product") != "chrome-for-testing-headless-shell"
):
    raise ValueError("browser identity unavailable")
story = read_json(story_path)
expected_path = [
    "Briefing",
    "Negotiation",
    "Commitment",
    "Resolution",
    "Result",
    "Complete",
]
if (
    story.get("status") != "completed"
    or story.get("evidence_class")
    != "real_browser_dom_plus_public_sdk_http_websocket"
    or story.get("phase_path") != expected_path
    or story.get("six_phase_order") is not True
    or story.get("browser_as_navigator") is not True
    or story.get("stale_head_rejected_and_new_id_resynced") is not True
    or story.get("reset_or_retained_catchup_installed") is not True
    or story.get("browser_precomplete_reveal_checked") is not True
    or story.get("credentials_in_built_assets_or_evidence") is not False
    or story.get("secrets") != "not_emitted"
    or story.get("final_replay", {}).get("verified") is not True
    or story.get("final_replay", {}).get("hash_parity", {}).get("verified") is not True
):
    raise ValueError("browser story result incomplete")
public_projection = story.get("public_projection")
if (
    not isinstance(public_projection, dict)
    or public_projection.get("broker_present") is not True
    or public_projection.get("commitment_count") != 2
    or public_projection.get("aggregate_outcome_present") is not True
):
    raise ValueError("browser story projection incomplete")

daemon = digest_file(daemon_path)
daemon["origin"] = "package:bin/worldstreamd" if package_mode == "1" else "source-build:target/debug/worldstreamd"
ui = tree_identity(ui_path, ("index.html", "compatibility-identity.json"))
ui["index_sha256"] = digest_file(Path(ui_path) / "index.html")["sha256"]
ui["origin"] = "package:ui" if package_mode == "1" else "source-build:web/console/dist"
sdk = tree_identity(sdk_path, ("worldstream_sdk/__init__.py",))
sdk["origin"] = "package:sdk/python/src" if package_mode == "1" else "source:sdk/python/src"
heist = tree_identity(
    heist_path,
    (
        "wave10_live/seed_browser_room.py",
        "wave10_live/run_browser_story.py",
        "wave10_live/run_absent_broker_live.py",
        "wave10_live/browser_trace_init.js",
    ),
)
heist["origin"] = "package:examples/heist" if package_mode == "1" else "source:examples/heist"
adapter = digest_file(adapter_path)
adapter.update({"name": "worldstream-cdp-browser", "protocol": "Chrome DevTools Protocol"})

if package_mode == "1":
    workspace = Path(workspace_path).resolve()
    resolved = [Path(value).resolve() for value in (daemon_path, ui_path, sdk_path, heist_path)]
    forbidden = [workspace / "target/debug", workspace / "web/console/dist"]
    if any(path == root or root in path.parents for path in resolved for root in forbidden):
        raise ValueError("source tree path entered package evidence")

dom_names = {
    "stale_rejection": "stale-body.txt",
    "precomplete_reveal": "precomplete-body.txt",
    "public_final": "public-body.txt",
    "participant_final": "participant-body.txt",
    "operator_final": "operator-body.txt",
    "replay_final": "replay-body.txt",
    "briefing": "briefing-phase.txt",
    "negotiation": "negotiation-phase.txt",
    "commitment": "commitment-phase.txt",
    "result": "result-phase.txt",
    "complete": "complete-phase.txt",
    "resync": "resync-live.txt",
    "browser_diagnostics": "browser-diagnostics.txt",
}
dom_evidence = {
    key: evidence_digest(evidence_root, value) for key, value in dom_names.items()
}
accepted_actions = {}
for action, evidence in {
    "inspect_clue": "inspect-accepted.txt",
    "publish_clue": "publish-accepted.txt",
    "propose_plan": "plan-accepted.txt",
    "commit_move": "nav-commit-accepted.txt",
    "acknowledge_result": "nav-ack-accepted.txt",
}.items():
    accepted_actions[action] = evidence_digest(evidence_root, evidence)

checks = {
    "browser_identity_verified": True,
    "catch_up_or_reset_installed": True,
    "embedded_ui_loaded": True,
    "final_reveal_dom_visible": True,
    "new_session_resynchronized": True,
    "package_bound_reference_clients": package_mode == "1",
    "package_bound_runtime": package_mode == "1",
    "precomplete_reveal_locked": True,
    "privacy_negative_dom_and_browser_channels": True,
    "replay_hashes_verified": True,
    "six_phase_story_complete": True,
    "stale_head_rejected": True,
    "typed_actions_accepted_in_dom": True,
}
report = {
    "schema": "worldstream/package-browser-heist/v1",
    "canonical_encoding": "utf8-sorted-key-compact-json-lf",
    "status": "pass",
    "release_evidence": package_mode == "1",
    "source_mode": "package-extracted" if package_mode == "1" else "source-build",
    "elapsed_ms": (int(finished_ns) - int(started_ns)) // 1_000_000,
    "browser": browser,
    "tools": {
        "adapter": adapter,
        "python": {
            "implementation": sys.implementation.name,
            "version": ".".join(str(item) for item in sys.version_info[:3]),
        },
    },
    "runtime": {
        "worldstreamd": daemon,
        "ui": ui,
        "sdk": sdk,
        "heist_reference_clients": heist,
    },
    "story": {
        "phase_path": story["phase_path"],
        "public_projection": public_projection,
        "final_replay": story["final_replay"],
    },
    "dom_evidence": dom_evidence,
    "typed_actions": accepted_actions,
    "checks": checks,
    "privacy": {
        "status": "pass",
        "private_canary_absent": True,
        "credentials_absent": True,
        "private_claim_absent_from_retained_evidence": True,
    },
}
if report["elapsed_ms"] <= 0:
    raise ValueError("invalid elapsed time")
destination = Path(destination)
descriptor, temporary_name = tempfile.mkstemp(
    prefix=f".{destination.name}.", dir=destination.parent
)
temporary = Path(temporary_name)
try:
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        json.dump(report, output, sort_keys=True, separators=(",", ":"))
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
printf 'status=passed\n'
printf 'surface=%s\n' "$surface"
printf 'base_url=%s\n' "$base_url"
printf 'evidence=real_browser_production_console_full_absent_broker_story\n'
printf 'six_phase_stale_resync_privacy_replay_hashes=verified\n'
