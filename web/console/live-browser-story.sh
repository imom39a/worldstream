#!/usr/bin/env bash
set -euo pipefail
workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$workspace_dir"
blocked() { printf 'blocked:%s\n' "$1" >&2; exit 2; }
command -v cmux >/dev/null 2>&1 || blocked browser_cli_unavailable
[[ -x target/debug/worldstreamd ]] || blocked worldstreamd_binary_missing
[[ -d web/console/dist ]] || blocked production_console_build_missing
command -v npm >/dev/null 2>&1 || blocked npm_unavailable_for_production_build
python_bin="${WORLDSTREAM_PYTHON:-$workspace_dir/sdk/python/.venv/bin/python}"
[[ -x "$python_bin" ]] || blocked python_sdk_runtime_missing
workspace_ref="${CMUX_WORKSPACE_ID:-}"
[[ -n "$workspace_ref" ]] || blocked cmux_workspace_context_missing
root="$(mktemp -d -t worldstream-live-browser.XXXXXX)"
chmod 700 "$root"
server_pid=""; console_pid=""; driver_pid=""; surface=""; claim=""
private_canary="required_tool_thermal_key"
preserve_artifacts="${KEEP_LIVE_BROWSER_ARTIFACTS:-0}"
story_timeout="${WORLDSTREAM_BROWSER_STORY_TIMEOUT:-240}"
[[ "$story_timeout" =~ ^[1-9][0-9]*$ ]] || blocked browser_story_timeout_invalid
window_ref="${CMUX_WINDOW_ID:-$(cmux identify --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["caller"]["window_ref"])')}"
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
if ! npm --prefix web/console run build >"$root/console-build.log" 2>&1; then
  tail -80 "$root/console-build.log" >&2 || true
  blocked production_console_build_failed
fi
bootstrap_secret="$root/authority.secret"; operator_bearer="$root/operator.bearer"
manifest="$root/browser-manifest.json"; driver_manifest="$root/driver-manifest.json"
python3 -c 'import os,secrets,sys; secret_path,bearer_path=sys.argv[1:]; secret=secrets.token_bytes(32); fd=os.open(secret_path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600); handle=os.fdopen(fd,"wb"); handle.write(secret); handle.flush(); os.fsync(handle.fileno()); handle.close(); open(bearer_path,"x",encoding="utf-8").write("wsb1:"+secret.hex()+"\n"); os.chmod(bearer_path,0o600)' "$bootstrap_secret" "$operator_bearer"
port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
base_url="http://127.0.0.1:$port"
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$bootstrap_secret" RUST_LOG="${WORLDSTREAM_LIVE_RUST_LOG:-warn}" target/debug/worldstreamd --data-dir "$data_dir" --bind "127.0.0.1:$port" >"$root/worldstreamd.log" 2>&1 &
server_pid="$!"
ready=0
for _ in $(seq 1 150); do
  if curl -fsS "$base_url/readyz" >/dev/null 2>&1; then ready=1; break; fi
  kill -0 "$server_pid" >/dev/null 2>&1 || blocked daemon_exited_before_ready
  sleep 0.1
done
[[ "$ready" = 1 ]] || blocked daemon_ready_timeout
if ! PYTHONPATH="$workspace_dir/sdk/python/src:$workspace_dir/examples/heist/wave10_live" "$python_bin" examples/heist/wave10_live/seed_browser_room.py --base-url "$base_url" --operator-bearer-file "$operator_bearer" --output "$manifest" --driver-output "$driver_manifest" >"$root/seed-summary.txt" 2>"$root/seed-error.txt"; then
  tail -40 "$root/seed-error.txt" >&2 || true
  blocked seed_browser_room_failed
fi
console_port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
python3 -m http.server "$console_port" --bind 127.0.0.1 --directory web/console/dist >"$root/console-http.log" 2>&1 &
console_pid="$!"
console_url="http://127.0.0.1:$console_port/?view=participant"
browser_open="$root/browser-open.json"
cmux --json browser open about:blank --workspace "$workspace_ref" --window "$window_ref" --focus false >"$browser_open" || blocked browser_surface_create_failed
surface="$(python3 -c 'import json,sys; value=json.load(open(sys.argv[1])); ref=value.get("surface_ref"); print(ref if isinstance(ref,str) else "")' "$browser_open")"
[[ "$surface" == surface:* ]] || blocked browser_surface_ref_missing
bootstrap_json="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])),separators=(",",":")))' "$manifest")"
init_script="window.__WORLDSTREAM_LIVE_SESSION__=$bootstrap_json;$(tr -d '\n' < examples/heist/wave10_live/browser_trace_init.js)"
cmux browser "$surface" addinitscript --script "$init_script" >"$root/browser-init.txt" || blocked browser_init_script_failed
cmux browser "$surface" navigate "$console_url" >"$root/browser-navigate.txt" || blocked browser_navigation_failed
cmux browser "$surface" wait --load-state complete --timeout-ms 15000 >"$root/browser-load.txt" || blocked browser_page_load_timeout
cmux browser "$surface" wait --text "Live session · live" --timeout-ms 30000 >"$root/browser-live.txt" || blocked live_session_did_not_reach_live
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
    if python3 -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("phase") == sys.argv[1] and value.get("roomSequence", 0) >= int(sys.argv[2]) else 1)' "$expected" "$minimum_sequence" <<<"$phase_probe"; then
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
assert_claim_absent() {
  local label="$1"
  local value="$2"
  [[ "$value" != *"$claim"* ]] || blocked "private_claim_leaked_$label"
  printf '%s\n' "$value" >"$root/$label.txt"
}
signal browser-ready; signal stale-ready
cmux browser "$surface" eval --script 'window.__WORLDSTREAM_HOLD_OBSERVATIONS__=true' >/dev/null
printf 'driver_launching timeout=%s\n' "$story_timeout" >"$root/driver.log"
PYTHONUNBUFFERED=1 PYTHONPATH="$workspace_dir/sdk/python/src:$workspace_dir/examples/heist/wave10_live" "$python_bin" -u examples/heist/wave10_live/run_browser_story.py \
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
cmux browser "$surface" eval --script 'window.__WORLDSTREAM_HOLD_OBSERVATIONS__=false;window.__WORLDSTREAM_OBSERVATION_QUEUE__=[]' >/dev/null
cmux browser "$surface" click '[data-testid="resync-session"]' >"$root/resync-click.txt" || blocked resync_control_missing
# The authorized claim is captured only after the resynchronized inspect action.
# Keep the diagnostic substitution safe if this pre-claim wait fails under set -u.
claim=""
resync_live=0
for _ in $(seq 1 300); do
  resync_probe="$(cmux browser "$surface" eval --script 'JSON.stringify({status:document.querySelector(".view-note strong")?.textContent || "", sockets:(window.__WORLDSTREAM_BROWSER_TRACE__ || []).filter((entry) => entry.kind === "websocket").length, syncAcks:(window.__WORLDSTREAM_BROWSER_TRACE__ || []).filter((entry) => entry.kind === "receive" && entry.type === "room.sync_acked").length})' 2>/dev/null || true)"
  if python3 -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("status") == "Live session · live" and value.get("sockets", 0) >= 2 and value.get("syncAcks", 0) >= 2 else 1)' <<<"$resync_probe"; then
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
cmux browser "$surface" click 'form[data-action-type="publish_clue"] button[type="submit"]' >"$root/publish-click.txt" || blocked publish_action_submit_missing
cmux browser "$surface" wait --text "Action sent; waiting for a durable server receipt." --timeout-ms 30000 >/dev/null || blocked publish_action_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/publish-accepted.txt" || blocked publish_action_not_accepted
signal navigator-published
wait_signal negotiation-open
wait_phase Negotiation negotiation-phase.txt 3 negotiation_phase_not_visible
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="route"]' service >"$root/plan-route.txt" || blocked propose_plan_form_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="entry_window"]' early >"$root/plan-window.txt" || blocked propose_plan_window_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="required_tool"]' thermal_key >"$root/plan-tool.txt" || blocked propose_plan_tool_missing
cmux browser "$surface" fill 'form[data-action-type="propose_plan"] input[name="extraction"]' boat >"$root/plan-extraction.txt" || blocked propose_plan_extraction_missing
cmux browser "$surface" click 'form[data-action-type="propose_plan"] button[type="submit"]' >"$root/plan-click.txt" || blocked propose_plan_submit_missing
cmux browser "$surface" wait --text "Action sent; waiting for a durable server receipt." --timeout-ms 30000 >/dev/null || blocked propose_plan_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/plan-accepted.txt" || blocked propose_plan_not_accepted
signal plan-proposed
wait_signal commitment-open
wait_phase Commitment commitment-phase.txt 6 commitment_phase_not_visible
wait_action_form commit_move commit_form_not_ready
wait_action_submit_enabled commit_move commit_submit_not_ready
plan_id="$(for _ in $(seq 1 120); do [[ -s "$root/plan-id" ]] && break; sleep 0.1; done; tr -d '\n' <"$root/plan-id")"
[[ -n "$plan_id" ]] || blocked public_plan_id_not_available
cmux browser "$surface" fill 'form[data-action-type="commit_move"] input[name="selected_plan_id"]' "$plan_id" >"$root/nav-commit-plan.txt" || blocked commit_form_missing
cmux browser "$surface" click 'form[data-action-type="commit_move"] button[type="submit"]' >"$root/nav-commit-click.txt" || blocked commit_submit_missing
cmux browser "$surface" wait --text "Action sent; waiting for a durable server receipt." --timeout-ms 30000 >/dev/null || blocked navigator_commit_not_sent
cmux browser "$surface" wait --text "Action accepted at the canonical Room Head." --timeout-ms 30000 >"$root/nav-commit-accepted.txt" || blocked navigator_commit_not_accepted
signal navigator-committed
wait_signal result-ready
wait_phase Result result-phase.txt 3 result_phase_not_visible
cmux browser "$surface" click '#replay-tab' >"$root/precomplete-replay-tab.txt" || blocked replay_tab_missing_precomplete
cmux browser "$surface" wait --text "Replay · read-only" --timeout-ms 15000 >"$root/precomplete-replay.txt" || blocked precomplete_replay_missing
precomplete="$(cmux browser "$surface" get text body)"
printf '%s\n' "$precomplete" | rg -q 'Locked until the Activity Phase is Complete' || blocked final_reveal_unlocked_precomplete
printf '%s\n' "$precomplete" | rg -q 'Historical authorization' || blocked historical_authorization_missing
cmux browser "$surface" click '#participant-tab' >/dev/null || blocked participant_tab_missing_for_ack
cmux browser "$surface" wait --text "acknowledge_result" --timeout-ms 30000 >/dev/null || blocked acknowledge_offer_missing
cmux browser "$surface" click 'form[data-action-type="acknowledge_result"] button[type="submit"]' >"$root/nav-ack-click.txt" || blocked acknowledge_submit_missing
cmux browser "$surface" wait --text "Action sent; waiting for a durable server receipt." --timeout-ms 30000 >/dev/null || blocked acknowledge_not_sent
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
if rg -n -P 'wsb1:[0-9a-f]{64}|wst1:[0-9a-f]{64}' web/console/dist >/dev/null 2>&1; then blocked credential_or_ticket_in_production_assets; fi
trace_text="$(cmux browser "$surface" eval --script 'JSON.stringify(window.__WORLDSTREAM_BROWSER_TRACE__ || [])')"
browser_diagnostics="$(cmux browser "$surface" errors list 2>/dev/null || true)$(cmux browser "$surface" console list 2>/dev/null || true)"
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
printf 'status=passed\n'
printf 'surface=%s\n' "$surface"
printf 'base_url=%s\n' "$base_url"
printf 'evidence=real_cmux_browser_production_console_full_absent_broker_story\n'
printf 'six_phase_stale_resync_privacy_replay_hashes=verified\n'
