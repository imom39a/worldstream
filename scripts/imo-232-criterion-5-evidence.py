#!/usr/bin/env python3
"""Run the finite local evidence lane for IMO-232 acceptance criterion five.

This deliberately runs focused production-path regressions only.  It is not a
substitute for IMO-235's 72-hour soak, a live PostgreSQL lane, or one-million
Transition qualification.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import Any


SCHEMA = "worldstream/imo-232-criterion-5-local-evidence/v1"
TIMEOUT_SECONDS = 300

TESTS: tuple[dict[str, Any], ...] = (
    {
        "id": "lost_action_reply_preserves_original_identity",
        "coverage": "A lost Action response restarts, reissues identical bytes, and resolves the original durable operation locally.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "assignment_mcp_actions",
            "lost_response_restart_reissues_identical_action_and_completed_retry_is_local",
        ),
    },
    {
        "id": "lost_activation_reply_advances_durably",
        "coverage": "A lost Activation offer claim advances durable state rather than replaying a stale selection.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "assignment_mcp_activations",
            "lost_offer_claim_advances_instead_of_replaying_the_stale_selection",
        ),
    },
    {
        "id": "lost_runner_reply_resumes_original_room_and_authority",
        "coverage": "A lost Runner reply reopens and resumes the original Room and authority, without minting a replacement operation.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "room_setup_operations",
            "lost_runner_reply_reopens_and_resumes_original_room_and_authority",
        ),
    },
    {
        "id": "acknowledged_transition_is_durable",
        "coverage": "A human acknowledgement materializes one agent intent once across duplicate delivery and recovery.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-sqlite",
            "--lib", "counter_v3_human_ack_durably_materializes_one_agent_intent_once_across_duplicate_and_recovery",
        ),
    },
    {
        "id": "runner_revocation_is_durable",
        "coverage": "Registered Runner controls are durably revoked and completion release remains fenced.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-sqlite",
            "--lib", "registered_runner_controls_completion_release_and_durable_revocation",
        ),
    },
    {
        "id": "runner_takeover_reconciles_leased_work",
        "coverage": "A bounded Runner restart delays retained leased work and advances it only after reconciliation.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "runner_attention",
            "leased_work_is_delayed_during_restart_and_advances_after_reconciliation",
        ),
    },
    {
        "id": "runner_assignment_restart_keeps_runner_separate",
        "coverage": "A seat Membership assignment is idempotent and restartable while retaining Runner separation.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "agent_profiles",
            "exact_seat_membership_assignment_is_idempotent_restartable_and_keeps_runner_separate",
        ),
    },
    {
        "id": "offline_timer_obligations_are_consumed_before_reactivation",
        "coverage": "Recovery stays catching-up until every due Timer obligation is consumed.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-sqlite",
            "--lib", "fixed_cutoff_recovery_stays_gated_until_each_due_obligation_is_consumed",
        ),
    },
    {
        "id": "cross_assignment_identity_does_not_disclose",
        "coverage": "An assignment-scoped identity conflict cannot disclose or redirect another assignment's Action state.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--test", "assignment_mcp_actions",
            "assignment_scoped_identity_conflicts_never_disclose_or_redirect",
        ),
    },
    {
        "id": "unauthenticated_history_replay_is_forbidden",
        "coverage": "The production HTTP replay route rejects an unauthenticated history request without returning a projection.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-server",
            "--lib", "replay_route_requires_auth_and_returns_exact_response_shape",
        ),
    },
    {
        "id": "historical_replay_reconstructs_authorized_membership_only",
        "coverage": "Authorized replay reconstructs historical Membership access and fences integrity, rather than exposing an unrestricted history view.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-sqlite",
            "--lib", "authorized_replay_reconstructs_historical_access_and_fences_integrity",
        ),
    },
    {
        "id": "runner_presence_is_host_authorized_and_bounded",
        "coverage": "The production Runner-presence route is host-authorized, bounded, and retains a safe disconnect state.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-server",
            "--lib", "runner_presence_is_host_authorized_bounded_and_retains_disconnect_state",
        ),
    },
    {
        "id": "credential_registry_restart_recovers_exact_identity",
        "coverage": "A bounded credential-registry restart recovers its exact secret reference and rejects an identity mutation.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p",
            "worldstream-studio-supervisor", "--lib",
            "model_provider_credentials::tests::interrupted_publication_recovers_before_and_after_target_without_accepting_changes",
        ),
    },
    {
        "id": "slow_consumer_closes_at_bound",
        "coverage": "A Session closes when its delivery buffer reaches the slow-consumer bound.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-core",
            "--lib", "buffer_overflow_closes_as_slow_consumer",
        ),
    },
    {
        "id": "internal_queue_is_bounded_and_shutdown_is_limited",
        "coverage": "The production telemetry queue is nonblocking, bounded, and has a limited shutdown path.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-server",
            "--lib", "telemetry::tests::queue_is_nonblocking_bounded_and_shutdown_is_time_limited",
        ),
    },
    {
        "id": "activation_attention_stays_bounded_without_losing_obligations",
        "coverage": "A 10,000-arrival local burst bounds refresh attention while retaining Timer and semantic-deadline obligations.",
        "command": (
            "cargo", "test", "--locked", "--release", "-p", "worldstream-sqlite",
            "--lib", "sustained_refresh_burst_keeps_live_attention_bounded_and_preserves_obligations",
        ),
    },
)

UNAVAILABLE_DIMENSIONS: tuple[dict[str, str], ...] = (
    {
        "dimension": "seventy_two_hour_soak",
        "status": "skipped",
        "reason": "Periodic termination, long offline intervals, and sustained credential renewal remain IMO-235's elapsed 72-hour schedule; short focused regressions are not equivalent.",
    },
)


def repository_revision(root: Path) -> str | None:
    result = subprocess.run(
        ("git", "rev-parse", "HEAD"), cwd=root, text=True, capture_output=True, check=False
    )
    return result.stdout.strip() if result.returncode == 0 else None


def run_test(root: Path, spec: dict[str, Any], dry_run: bool) -> dict[str, Any]:
    command = list(spec["command"])
    if "soak" in " ".join(command).lower():
        raise RuntimeError("IMO-232 criterion-5 runner must not execute a soak")
    row: dict[str, Any] = {
        "id": spec["id"],
        "coverage": spec["coverage"],
        "command": command,
    }
    if dry_run:
        row["status"] = "planned"
        return row
    started = time.monotonic()
    completed = subprocess.run(
        command,
        cwd=root,
        text=True,
        capture_output=True,
        timeout=TIMEOUT_SECONDS,
        check=False,
        env={**os.environ, "CARGO_TERM_COLOR": "never"},
    )
    output = completed.stdout + completed.stderr
    row.update(
        {
            "status": "pass" if completed.returncode == 0 else "failed",
            "exit_code": completed.returncode,
            "duration_ms": round((time.monotonic() - started) * 1000),
            "output_sha256": hashlib.sha256(output.encode()).hexdigest(),
        }
    )
    if completed.returncode != 0:
        row["output_tail"] = output[-4000:]
    return row


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    rows = [run_test(root, spec, args.dry_run) for spec in TESTS]
    passed = all(row["status"] in {"pass", "planned"} for row in rows)
    report = {
        "schema": SCHEMA,
        "release_evidence": False,
        "qualification_status": "partial",
        "scope": "finite_local_production_regressions",
        "repository_revision": repository_revision(root),
        "execution": "dry_run" if args.dry_run else "executed",
        "test_timeout_seconds": TIMEOUT_SECONDS,
        "tests": rows,
        "unavailable_dimensions": list(UNAVAILABLE_DIMENSIONS),
        "pass": passed,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
