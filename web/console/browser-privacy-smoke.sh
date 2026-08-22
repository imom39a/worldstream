#!/bin/sh
set -eu

browser="${CHROME_BIN:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
url="${CONSOLE_URL:-http://127.0.0.1:5173/}"
preview_pid=""
preview_log=""

cleanup() {
  if [ -n "$preview_pid" ] && kill -0 "$preview_pid" >/dev/null 2>&1; then
    kill "$preview_pid" >/dev/null 2>&1 || true
    wait "$preview_pid" 2>/dev/null || true
  fi
  if [ -n "$preview_log" ]; then
    rm -f "$preview_log"
  fi
}
trap cleanup EXIT

if [ ! -x "$browser" ]; then
  echo "browser unavailable: set CHROME_BIN to a headless Chromium-compatible browser" >&2
  exit 2
fi

if [ "${START_PREVIEW:-0}" = "1" ]; then
  preview_log="$(mktemp -t worldstream-console-preview.XXXXXX)"
  port="${CONSOLE_PORT:-4173}"
  (cd "$(dirname "$0")" && exec pnpm exec vite preview --host 127.0.0.1 --port "$port" >"$preview_log" 2>&1) &
  preview_pid="$!"
  url="http://127.0.0.1:$port/"
  for _ in $(seq 1 50); do
    if curl -fsS "$url" >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
fi

for view in public participant operator replay; do
  dom="$("$browser" --headless=new --disable-gpu --no-sandbox --virtual-time-budget=1500 --dump-dom "${url}?scenario=healthy&view=$view" 2>/dev/null)"
  printf '%s\n' "$dom" | rg -q "Agent Heist console"
  case "$view" in
    public) printf '%s\n' "$dom" | rg -q "Public projection"; printf '%s\n' "$dom" | rg -q "individual commitments remain withheld" ;;
    participant) printf '%s\n' "$dom" | rg -q "Current Action Offers"; printf '%s\n' "$dom" | rg -q "based_on_room_seq"; printf '%s\n' "$dom" | rg -q "Submit unavailable" ;;
    operator) printf '%s\n' "$dom" | rg -q "Bounded diagnostics"; printf '%s\n' "$dom" | rg -q "No invocation payloads" ;;
    replay) printf '%s\n' "$dom" | rg -q "Replay · read-only"; printf '%s\n' "$dom" | rg -q "Mutation controls unavailable"; printf '%s\n' "$dom" | rg -q 'type="range"' ;;
  esac
  if printf '%s\n' "$dom" | rg -qi "navigator-private-clue|sealed-value|private_clue|own_commitment|chain-of-thought|Bearer[[:space:]]|wst1:|password|api[_-]?key|raw (core|activity) state"; then
    printf '%s\n' "privacy failure: forbidden participant/private material reached the $view browser DOM" >&2
    exit 1
  fi
done

for state in loading catching-up faulted quarantined; do
  dom="$("$browser" --headless=new --disable-gpu --no-sandbox --virtual-time-budget=1500 --dump-dom "${url}?scenario=$state&view=participant" 2>/dev/null)"
  case "$state" in
    loading) printf '%s\n' "$dom" | rg -q "Loading"; printf '%s\n' "$dom" | rg -q "Current Action Offers withheld" ;;
    catching-up) printf '%s\n' "$dom" | rg -q "Catching up"; printf '%s\n' "$dom" | rg -q "Current Action Offers withheld" ;;
    faulted) printf '%s\n' "$dom" | rg -q "Faulted"; printf '%s\n' "$dom" | rg -q "Canonical mutation remains disabled" ;;
    quarantined) printf '%s\n' "$dom" | rg -q "Quarantined · fail closed"; printf '%s\n' "$dom" | rg -q "Participant Projection unavailable" ;;
  esac
  if [ "$state" = "loading" ] || [ "$state" = "catching-up" ]; then
    public_dom="$("$browser" --headless=new --disable-gpu --no-sandbox --virtual-time-budget=1500 --dump-dom "${url}?scenario=$state&view=public" 2>/dev/null)"
    replay_dom="$("$browser" --headless=new --disable-gpu --no-sandbox --virtual-time-budget=1500 --dump-dom "${url}?scenario=$state&view=replay" 2>/dev/null)"
    printf '%s\n' "$public_dom" | rg -q "Public Projection unavailable"
    if printf '%s\n' "$public_dom" | rg -q "Canal lift / service window"; then exit 1; fi
    printf '%s\n' "$replay_dom" | rg -q "Historical Replay unavailable"
    if printf '%s\n' "$replay_dom" | rg -q "Canonical story replay"; then exit 1; fi
  fi
done

complete_dom="$("$browser" --headless=new --disable-gpu --no-sandbox --virtual-time-budget=1500 --dump-dom "${url}?scenario=complete&view=replay" 2>/dev/null)"
printf '%s\n' "$complete_dom" | rg -q "The activity is terminal"
printf '%s\n' "$complete_dom" | rg -q '>Available</span>'

printf '%s\n' "browser privacy smoke passed: public, participant, operator, Replay, recovery, fault, quarantine, and terminal fixture routes were inspected without credentials"
