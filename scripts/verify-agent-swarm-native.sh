#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Agent Swarm native smoke must run on macOS.' >&2
  exit 2
fi

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-agent-swarm-smoke.XXXXXX")"
cleanup_smoke() {
  if [[ -f "$smoke_root/managed-processes-stopped" ]]; then
    rm -rf -- "$smoke_root"
  else
    printf 'Agent Swarm smoke state retained for authenticated cleanup: %s\n' "$smoke_root" >&2
  fi
}
trap cleanup_smoke EXIT

cd "$workspace_dir"
cargo test --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime
cargo test --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime --test tui_pty
cargo build --quiet --locked -p worldstream-server -p worldstream-studio-supervisor --bins
cargo build --quiet --locked -p worldstream-agent-swarm --features managed-local-runtime --bins
expected='{"status":"ok","backend":"managed_local","rooms":2,"reopened":true,"reviewed_result":true,"reopened_reviewed_result":true,"late_output_fenced":true,"changed_input_revalidated":true,"review_correction_revalidated":true,"review_dispute_visible":true,"resource_conflict_reconciled":true,"human_steering_reassigned":true,"progress_review_claimed":true,"automatic_progress_review":true,"automatic_progress_review_waited_for_capacity":true,"bounded_realignments_escalated":true,"competing_claim":true,"duplicate_retry":true,"artifact_resolved":true,"coordinator_worker_contribution":true,"guarded_report_check":true,"reviewed_code_change":true,"code_change_conflict_preserved":true,"room_code_change_result":true,"native_shared_capacity":true,"native_progress_review_priority":true,"native_budget_pause":true,"native_effect_recovery":true}'
actual="$(uv run --project sdk/python --python 3.14.7 python scripts/verify-agent-swarm-managed.py \
  --workspace "$workspace_dir" --root "$smoke_root")"
if [[ "$actual" != "$expected" ]]; then
  printf 'unexpected Agent Swarm smoke receipt: %s\n' "$actual" >&2
  exit 1
fi

printf '%s\n' 'Agent Swarm native macOS smoke passed.'
