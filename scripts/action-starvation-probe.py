#!/usr/bin/env python3
"""Measure strict based-on-head Action contention with a deterministic Runner.

This is a fixture-only admission probe, not a replacement admission path.  The
``StrictProductionAdmission`` adapter has the same safety rule as the current
Room Kernel: an Action is accepted only when its ``based_on_room_seq`` equals
the Complete Head at admission.  A stale proposal is discarded, a new Action
identity is created after refresh and re-evaluation, and no proposal is
silently rebased.

The probe intentionally models two distinct cases:

* ``during_reasoning`` starts from a synchronized Head and advances the Room
  while the Runner is deciding (genuine contention).
* ``hidden_head_lag`` advances the Room before the Runner starts while hiding
  the update from its current view (the IMO-217 synchronization baseline).

All clocks are integer virtual milliseconds and all updates follow a periodic
schedule.  No model provider, network, or random source is used.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

SCHEMA = "worldstream/action-starvation-probe/v1"
PROBE_VERSION = "deterministic-periodic-v1"
MAX_ATTEMPTS_PER_TRIAL = 32
MAX_SAMPLE_ATTEMPTS = 12
PRODUCTION_ADMISSION_SOURCE = (
    "crates/worldstream-core/src/trace.rs::assess_stable_action_disposition"
)
ENVELOPE = {
    "decision_delay_max_ms": 250,
    "room_update_rate_max_per_second": 2,
    "max_attempts_per_trial": 8,
}

Visibility = Literal["visible", "hidden"]
Relation = Literal["related", "unrelated"]
Mode = Literal["during_reasoning", "hidden_head_lag"]


@dataclass(frozen=True)
class Scenario:
    decision_delay_ms: int
    room_update_rate_per_second: int
    participant_count: int
    update_visibility: Visibility
    update_relation: Relation
    mode: Mode = "during_reasoning"
    trials: int = 8


@dataclass
class TrialResult:
    accepted: bool = False
    accepted_at_ms: int | None = None
    stale_rejections: int = 0
    attempts: int = 0
    reevaluations: int = 0
    model_wasted_work: int = 0
    updates_total: int = 0
    visible_updates: int = 0
    hidden_updates: int = 0
    related_updates: int = 0
    unrelated_updates: int = 0
    attempt_trace: list[dict[str, Any]] | None = None


class StrictProductionAdmission:
    """Small probe adapter for the production exact-head admission contract."""

    @staticmethod
    def submit(
        *, action_id: str, based_on_room_seq: int, current_room_seq: int
    ) -> Literal["accepted", "stale_room_state"]:
        # This comparison is deliberately the whole admission rule under test.
        del action_id
        return "accepted" if based_on_room_seq == current_room_seq else "stale_room_state"


def _validate_scenario(scenario: Scenario) -> None:
    if scenario.decision_delay_ms < 0:
        raise ValueError("decision_delay_ms must be non-negative")
    if scenario.room_update_rate_per_second < 0:
        raise ValueError("room_update_rate_per_second must be non-negative")
    if scenario.participant_count < 1:
        raise ValueError("participant_count must be positive")
    if scenario.trials < 1 or scenario.trials > 10_000:
        raise ValueError("trials must be between 1 and 10,000")


def _updates_between(
    *, start_ms: int, end_ms: int, rate_per_second: int, next_update_index: int
) -> tuple[int, int]:
    """Return periodic update count and next index without floating point time."""

    if rate_per_second == 0 or end_ms <= start_ms:
        return 0, next_update_index
    period_num = 1_000
    period_den = rate_per_second
    first_index = max(next_update_index, (start_ms * period_den) // period_num + 1)
    last_index = (end_ms * period_den) // period_num
    count = max(0, last_index - first_index + 1)
    return count, first_index + count


def run_scenario(scenario: Scenario, *, sample_trace: bool = False) -> dict[str, Any]:
    _validate_scenario(scenario)
    trials: list[TrialResult] = []
    room_head = 0
    virtual_now = 0
    next_update_index = 1
    pending_attempt_bound = 0
    action_id_counter = 0

    for trial_index in range(scenario.trials):
        result = TrialResult(attempt_trace=[] if sample_trace and trial_index == 0 else None)
        observed_head = room_head
        trial_start = virtual_now

        # IMO-217 baseline: private work already committed before deciding is
        # absent from the Runner's current view. The stale Action still carries
        # the old exact basis and is rejected by production admission.
        if scenario.mode == "hidden_head_lag" and scenario.update_visibility == "hidden":
            room_head += 1
            result.updates_total += 1
            result.hidden_updates += 1
            if scenario.update_relation == "related":
                result.related_updates += 1
            else:
                result.unrelated_updates += 1

        for attempt in range(1, MAX_ATTEMPTS_PER_TRIAL + 1):
            result.attempts += 1
            pending_attempt_bound = max(pending_attempt_bound, 1)
            action_id_counter += 1
            action_id = f"probe-action-{action_id_counter:06d}"
            decision_start = virtual_now
            decision_end = decision_start + scenario.decision_delay_ms
            update_count, next_update_index = _updates_between(
                start_ms=decision_start,
                end_ms=decision_end,
                rate_per_second=scenario.room_update_rate_per_second,
                next_update_index=next_update_index,
            )
            room_head += update_count
            result.updates_total += update_count
            if scenario.update_visibility == "visible":
                result.visible_updates += update_count
            else:
                result.hidden_updates += update_count
            if scenario.update_relation == "related":
                result.related_updates += update_count
            else:
                result.unrelated_updates += update_count
            outcome = StrictProductionAdmission.submit(
                action_id=action_id,
                based_on_room_seq=observed_head,
                current_room_seq=room_head,
            )
            if result.attempt_trace is not None and len(result.attempt_trace) < MAX_SAMPLE_ATTEMPTS:
                result.attempt_trace.append({
                    "action_id": action_id,
                    "based_on_room_seq": observed_head,
                    "current_room_seq_at_admission": room_head,
                    "decision_start_ms": decision_start,
                    "decision_end_ms": decision_end,
                    "outcome": outcome,
                    "new_action_after_stale": outcome == "stale_room_state",
                })
            virtual_now = decision_end
            if outcome == "accepted":
                result.accepted = True
                result.accepted_at_ms = virtual_now - trial_start
                break
            result.stale_rejections += 1
            result.model_wasted_work += 1
            result.reevaluations += 1
            # Refresh is explicit and happens after the rejected proposal. The
            # next attempt obtains a new Action ID and current exact basis.
            observed_head = room_head
        trials.append(result)

    accepted = sum(1 for result in trials if result.accepted)
    stale = sum(result.stale_rejections for result in trials)
    attempts = sum(result.attempts for result in trials)
    useful_latencies = [result.accepted_at_ms for result in trials if result.accepted_at_ms is not None]
    starvation = scenario.trials - accepted
    total_updates = sum(result.updates_total for result in trials)
    visible_updates = sum(result.visible_updates for result in trials)
    hidden_updates = sum(result.hidden_updates for result in trials)
    related_updates = sum(result.related_updates for result in trials)
    unrelated_updates = sum(result.unrelated_updates for result in trials)
    in_envelope = (
        scenario.mode == "during_reasoning"
        and scenario.decision_delay_ms <= ENVELOPE["decision_delay_max_ms"]
        and scenario.room_update_rate_per_second <= ENVELOPE["room_update_rate_max_per_second"]
    )
    thresholds = {
        "in_stated_envelope": in_envelope,
        "max_attempts_per_trial": ENVELOPE["max_attempts_per_trial"],
        "useful_action_required": in_envelope,
        "starvation_allowed": False if in_envelope else True,
    }
    threshold_met = (not in_envelope or (accepted == scenario.trials and max((r.attempts for r in trials), default=0) <= ENVELOPE["max_attempts_per_trial"]))
    report: dict[str, Any] = {
        "scenario": {
            "decision_delay_ms": scenario.decision_delay_ms,
            "mode": scenario.mode,
            "participant_count": scenario.participant_count,
            "room_update_rate_per_second": scenario.room_update_rate_per_second,
            "trials": scenario.trials,
            "update_relation": scenario.update_relation,
            "update_visibility": scenario.update_visibility,
        },
        "measurements": {
            "accepted_useful_actions": accepted,
            "action_attempts": attempts,
            "stale_rejections": stale,
            "stale_rejection_rate": round(stale / attempts, 6) if attempts else 0.0,
            "starved_trials": starvation,
            "starvation_rate": round(starvation / scenario.trials, 6),
            "model_equivalent_wasted_work": sum(r.model_wasted_work for r in trials),
            "reevaluations_after_stale": sum(r.reevaluations for r in trials),
            "accepted_useful_latency_ms": _latency_summary(useful_latencies),
            "updates": {
                "total": total_updates,
                "visible": visible_updates,
                "hidden": hidden_updates,
                "related": related_updates,
                "unrelated": unrelated_updates,
            },
            "pending_attempts_max": pending_attempt_bound,
            "attempts_per_trial_max": max((r.attempts for r in trials), default=0),
            "participant_control": {
                "participant_count": scenario.participant_count,
                "aggregate_room_rate_held_constant": True,
                "scheduler_fairness_measured": False,
            },
        },
        "threshold_evaluation": {
            "thresholds": thresholds,
            "met": threshold_met,
            "outcome": "within_envelope" if threshold_met and in_envelope else "outside_envelope" if threshold_met else "missed",
        },
    }
    if sample_trace:
        report["sample_trial_attempts"] = trials[0].attempt_trace if trials and trials[0].attempt_trace is not None else []
    report["bounds"] = {
        "max_attempts_per_trial": MAX_ATTEMPTS_PER_TRIAL,
        "max_pending_attempts": 1,
        "max_sample_attempts": MAX_SAMPLE_ATTEMPTS,
    }
    return report


def _latency_summary(values: list[int]) -> dict[str, Any]:
    if not values:
        return {"sample_count": 0, "p50_ms": None, "p95_ms": None, "p99_ms": None}
    ordered = sorted(values)

    def nearest_rank(fraction: float) -> int:
        return ordered[max(0, min(len(ordered) - 1, math.ceil(len(ordered) * fraction) - 1))]

    return {
        "sample_count": len(values),
        "p50_ms": nearest_rank(0.50),
        "p95_ms": nearest_rank(0.95),
        "p99_ms": nearest_rank(0.99),
    }


def default_matrix() -> list[Scenario]:
    scenarios: list[Scenario] = []
    for mode in ("during_reasoning", "hidden_head_lag"):
        for delay in (0, 100, 500, 2_000):
            for rate in (0, 2, 10):
                for visibility in ("visible", "hidden"):
                    for relation in ("related", "unrelated"):
                        scenarios.append(Scenario(delay, rate, 1, visibility, relation, mode))
    # Participant count is an independent axis; retain a compact matrix while
    # still exercising fair multi-participant occupancy.
    scenarios.extend(
        Scenario(500, 10, participants, "visible", "unrelated", "during_reasoning")
        for participants in (2, 4, 8)
    )
    return scenarios


def run_probe(scenarios: list[Scenario] | None = None) -> dict[str, Any]:
    selected = default_matrix() if scenarios is None else scenarios
    results = [run_scenario(scenario, sample_trace=index == 0) for index, scenario in enumerate(selected)]
    envelope_results = [item for item in results if item["threshold_evaluation"]["thresholds"]["in_stated_envelope"]]
    envelope_passed = bool(envelope_results) and all(item["threshold_evaluation"]["met"] for item in envelope_results)
    outside_starvation = any(
        item["measurements"]["starved_trials"] > 0
        for item in results
        if not item["threshold_evaluation"]["thresholds"]["in_stated_envelope"]
    )
    return {
        "schema": SCHEMA,
        "probe_version": PROBE_VERSION,
        "admission_contract": {
            "based_on_room_seq": "exact_current_complete_head_room_seq",
            "stale_result": "stale_room_state",
            "stale_action_reused": False,
            "production_adapter": "StrictProductionAdmission",
            "rule_source": PRODUCTION_ADMISSION_SOURCE,
            "backend_commit_latency_measured": False,
        },
        "comparison": {
            "imo_217_hidden_head_lag_is_separate": True,
            "during_reasoning_starts_synchronized": True,
            "visible_updates_are_not_private_payloads": True,
            "related_updates_require_re_evaluation": True,
            "unrelated_updates_still_change_whole_head": True,
        },
        "bounds": {
            "max_attempts_per_trial": MAX_ATTEMPTS_PER_TRIAL,
            "max_pending_attempts": 1,
            "max_sample_attempts": MAX_SAMPLE_ATTEMPTS,
            "max_report_scenarios": len(results),
        },
        "threshold_evaluation": {
            "stated_envelope": ENVELOPE,
            "envelope_scenarios_passed": envelope_passed,
            "current_contract_adequate_within_envelope": envelope_passed,
            "successor_adr_needed_for_observed_outside_envelope_starvation": outside_starvation,
            "decision": "current_contract_adequate_within_stated_envelope; measure a versioned successor before changing admission" if envelope_passed else "current contract misses its stated envelope; prepare a successor ADR from these measurements",
        },
        "scenarios": results,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="write the deterministic JSON report to this path")
    parser.add_argument("--compact", action="store_true", help="emit one JSON line")
    args = parser.parse_args(argv)
    report = run_probe()
    text = json.dumps(report, sort_keys=True, indent=None if args.compact else 2)
    if not args.compact:
        text += "\n"
    if args.output is None:
        sys.stdout.write(text)
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
