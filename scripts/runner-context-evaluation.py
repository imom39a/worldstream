#!/usr/bin/env python3
"""Run a bounded, model-backed comparison of late-join context strategies.

The fixture is synthetic but follows the public late-join reference Pack's
assessment rule. Runtime correctness and model policy quality remain separate:
this script scores only the model's proposed typed Actions against a fixed
oracle and never submits them to a Room.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "worldstream.runner-context-evaluation.v1"
DEFAULT_MODEL = "gpt-5.6-luna"
PROMPT_BUDGET_BYTES = 24 * 1024
OUTPUT_SCHEMA: dict[str, Any] = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "additionalProperties": False,
    "required": ["case_results"],
    "properties": {
        "case_results": {
            "type": "array",
            "minItems": 4,
            "maxItems": 4,
            "items": {
                "type": "object",
                "additionalProperties": False,
                "required": ["case_id", "actions", "abstain_reason"],
                "properties": {
                    "case_id": {"type": "string"},
                    "actions": {
                        "type": "array",
                        "maxItems": 4,
                        "items": {
                            "type": "object",
                            "additionalProperties": False,
                            "required": [
                                "action_type",
                                "work_id",
                                "expected_work_revision",
                                "arrival_version",
                                "departure_version",
                                "claim",
                                "reason_code",
                                "evidence_ids",
                            ],
                            "properties": {
                                "action_type": {"type": "string"},
                                "work_id": {"type": "string"},
                                "expected_work_revision": {"type": "integer"},
                                "arrival_version": {"type": "integer"},
                                "departure_version": {"type": "integer"},
                                "claim": {"type": "string"},
                                "reason_code": {"type": "string"},
                                "evidence_ids": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                },
                            },
                        },
                    },
                    "abstain_reason": {"type": ["string", "null"]},
                },
            },
        }
    },
}


@dataclass(frozen=True)
class ExpectedAction:
    work_id: str
    expected_work_revision: int
    arrival_version: int
    departure_version: int
    claim: str
    reason_code: str
    evidence_ids: tuple[str, ...]


def fact(
    connection_id: str,
    *,
    arrival_minute: int,
    arrival_version: int,
    departure_minute: int,
    departure_version: int,
    minimum_transfer_minutes: int,
) -> dict[str, Any]:
    return {
        "connection_id": connection_id,
        "arrival_minute": arrival_minute,
        "arrival_version": arrival_version,
        "departure_minute": departure_minute,
        "departure_version": departure_version,
        "minimum_transfer_minutes": minimum_transfer_minutes,
    }


FACT_C17 = fact(
    "C17",
    arrival_minute=640,
    arrival_version=3,
    departure_minute=650,
    departure_version=1,
    minimum_transfer_minutes=25,
)
FACT_C42 = fact(
    "C42",
    arrival_minute=600,
    arrival_version=1,
    departure_minute=650,
    departure_version=1,
    minimum_transfer_minutes=25,
)
FACT_C88 = fact(
    "C88",
    arrival_minute=710,
    arrival_version=5,
    departure_minute=725,
    departure_version=2,
    minimum_transfer_minutes=20,
)


EXPECTED: dict[str, tuple[ExpectedAction, ...]] = {
    "offline-revision": (
        ExpectedAction(
            "work-C17",
            3,
            3,
            1,
            "connection_at_risk",
            "transfer_time_insufficient",
            ("source:C17:a3:d1",),
        ),
    ),
    "two-obligations": (
        ExpectedAction(
            "work-C17",
            3,
            3,
            1,
            "connection_at_risk",
            "transfer_time_insufficient",
            ("source:C17:a3:d1",),
        ),
        ExpectedAction(
            "work-C42",
            1,
            1,
            1,
            "connection_feasible",
            "transfer_time_sufficient",
            ("source:C42:a1:d1",),
        ),
    ),
    "conflicting-old-evidence": (
        ExpectedAction(
            "work-C88",
            5,
            5,
            2,
            "connection_at_risk",
            "transfer_time_insufficient",
            ("source:C88:a5:d2",),
        ),
    ),
    "no-offer": (),
}


def projection_case(case_id: str) -> dict[str, Any]:
    if case_id == "offline-revision":
        return {
            "case_id": case_id,
            "complete_head": 100_003,
            "role": "reviewer",
            "projection": {
                "connections": [{**FACT_C17, "evidence_id": "source:C17:a3:d1"}],
                "open_work": [{"work_id": "work-C17", "connection_id": "C17", "revision": 3, "status": "needs_assessment"}],
            },
            "action_offers": ["record_assessment"],
        }
    if case_id == "two-obligations":
        return {
            "case_id": case_id,
            "complete_head": 210_001,
            "role": "reviewer",
            "projection": {
                "connections": [
                    {**FACT_C17, "evidence_id": "source:C17:a3:d1"},
                    {**FACT_C42, "evidence_id": "source:C42:a1:d1"},
                ],
                "open_work": [
                    {"work_id": "work-C17", "connection_id": "C17", "revision": 3, "status": "needs_assessment"},
                    {"work_id": "work-C42", "connection_id": "C42", "revision": 1, "status": "needs_assessment"},
                ],
            },
            "action_offers": ["record_assessment"],
        }
    if case_id == "conflicting-old-evidence":
        return {
            "case_id": case_id,
            "complete_head": 330_005,
            "role": "reviewer",
            "projection": {
                "connections": [{**FACT_C88, "evidence_id": "source:C88:a5:d2"}],
                "open_work": [{"work_id": "work-C88", "connection_id": "C88", "revision": 5, "status": "needs_assessment"}],
            },
            "action_offers": ["record_assessment"],
            "historical_evidence": {
                "evidence_id": "assessment:C88:rev4",
                "validity": "superseded",
                "claim": "connection_feasible",
            },
        }
    if case_id == "no-offer":
        return {
            "case_id": case_id,
            "complete_head": 440_009,
            "role": "reviewer",
            "projection": {
                "connections": [{**FACT_C42, "evidence_id": "source:C42:a1:d1"}],
                "open_work": [{
                    "work_id": "work-C42",
                    "connection_id": "C42",
                    "revision": 1,
                    "status": "assessed",
                    "last_assessment": {"validity": "current", "claim": "connection_feasible"},
                }],
            },
            "action_offers": [],
        }
    raise AssertionError(case_id)


def recent_history_case(case_id: str) -> dict[str, Any]:
    if case_id == "offline-revision":
        events = [
            {"seq": 100_001, "event": "assessment_accepted", "work_id": "work-C17", "revision": 2, "claim": "connection_feasible"},
            {"seq": 100_002, "event": "runner_offline"},
            {"seq": 100_003, "event": "source_fact_replaced", **FACT_C17, "work_revision": 3, "evidence_id": "source:C17:a3:d1"},
        ]
    elif case_id == "two-obligations":
        events = [
            {"seq": 209_998, "event": "unrelated_timer"},
            {"seq": 209_999, "event": "unrelated_member_joined"},
            {"seq": 210_000, "event": "source_fact_recorded", **FACT_C42, "work_revision": 1, "evidence_id": "source:C42:a1:d1"},
            {"seq": 210_001, "event": "assessment_required", "work_id": "work-C42"},
        ]
    elif case_id == "conflicting-old-evidence":
        events = [
            {"seq": 330_002, "event": "assessment_accepted", "work_id": "work-C88", "revision": 4, "claim": "connection_feasible", "evidence_id": "assessment:C88:rev4"},
            {"seq": 330_003, "event": "source_fact_replaced", **FACT_C88, "work_revision": 5, "evidence_id": "source:C88:a5:d2"},
            {"seq": 330_004, "event": "assessment_superseded", "work_id": "work-C88", "revision": 4},
            {"seq": 330_005, "event": "assessment_required", "work_id": "work-C88", "work_revision": 5},
        ]
    elif case_id == "no-offer":
        events = [
            {"seq": 440_007, "event": "source_fact_recorded", **FACT_C42, "work_revision": 1, "evidence_id": "source:C42:a1:d1"},
            {"seq": 440_008, "event": "assessment_accepted", "work_id": "work-C42", "revision": 1, "claim": "connection_feasible"},
            {"seq": 440_009, "event": "projection_replaced", "action_offers": []},
        ]
    else:
        raise AssertionError(case_id)
    return {
        "case_id": case_id,
        "context_kind": "recent_history",
        "bounded_recent_events": events,
        "notice": "Events older than this retained window are unavailable.",
    }


def summary_retrieval_case(case_id: str) -> dict[str, Any]:
    projection = projection_case(case_id)
    if case_id == "offline-revision":
        summary = "One assessment reopened after a source revision while the Runner was offline."
        evidence = [{**FACT_C17, "work_id": "work-C17", "work_revision": 3, "evidence_id": "source:C17:a3:d1"}]
        offers = ["record_assessment"]
    elif case_id == "two-obligations":
        summary = "Two assessments are open: work-C17 revision 3 and work-C42 revision 1."
        evidence = [
            {**FACT_C17, "work_id": "work-C17", "work_revision": 3, "evidence_id": "source:C17:a3:d1"},
            {**FACT_C42, "work_id": "work-C42", "work_revision": 1, "evidence_id": "source:C42:a1:d1"},
        ]
        offers = ["record_assessment"]
    elif case_id == "conflicting-old-evidence":
        summary = "The last assessment said feasible, but work-C88 reopened at revision 5 after newer source evidence."
        evidence = [
            {"evidence_id": "assessment:C88:rev4", "validity": "superseded", "claim": "connection_feasible"},
            {**FACT_C88, "work_id": "work-C88", "work_revision": 5, "evidence_id": "source:C88:a5:d2"},
        ]
        offers = ["record_assessment"]
    elif case_id == "no-offer":
        summary = "work-C42 is assessed and current; no assessment is offered."
        evidence = [{**FACT_C42, "work_id": "work-C42", "work_revision": 1, "evidence_id": "source:C42:a1:d1"}]
        offers = []
    else:
        raise AssertionError(case_id)
    return {
        "case_id": case_id,
        "context_kind": "summary_plus_retrieval",
        "fallible_summary": summary,
        "retrieved_authorized_evidence": evidence,
        "action_offers": offers,
        "complete_head": projection["complete_head"],
    }


def contexts(strategy: str) -> list[dict[str, Any]]:
    case_ids = list(EXPECTED)
    if strategy == "recent_history":
        return [recent_history_case(case_id) for case_id in case_ids]
    if strategy == "summary_plus_retrieval":
        return [summary_retrieval_case(case_id) for case_id in case_ids]
    if strategy == "projection_explicit_work":
        return [projection_case(case_id) for case_id in case_ids]
    raise AssertionError(strategy)


def make_prompt(strategy: str) -> str:
    payload = contexts(strategy)
    return "\n".join(
        [
            "You are a bounded external Runner evaluating four independent WorldStream late-join cases.",
            "Do not use tools. Return only the JSON object required by the supplied output schema.",
            "Treat each case independently; never carry facts from one case into another.",
            "The only supported Action is record_assessment and it may be proposed only when the case explicitly offers it.",
            "For each offered open work item, use the current work/source revisions. Transfer time is departure_minute - arrival_minute.",
            "If transfer time is at least minimum_transfer_minutes, claim connection_feasible with reason transfer_time_sufficient; otherwise claim connection_at_risk with reason transfer_time_insufficient.",
            "Never propose close_work. Superseded assessments are historical evidence, not current facts. Cite the exact current source evidence_id used.",
            "If the supplied context does not identify all current open work or lacks current source facts, omit unsupported Actions and give a concise abstain_reason.",
            f"Context strategy: {strategy}",
            json.dumps(payload, sort_keys=True, separators=(",", ":")),
        ]
    )


def normalize_action(action: dict[str, Any]) -> tuple[Any, ...]:
    return (
        action.get("action_type"),
        action.get("work_id"),
        action.get("expected_work_revision"),
        action.get("arrival_version"),
        action.get("departure_version"),
        action.get("claim"),
        action.get("reason_code"),
    )


def expected_tuple(expected: ExpectedAction) -> tuple[Any, ...]:
    return (
        "record_assessment",
        expected.work_id,
        expected.expected_work_revision,
        expected.arrival_version,
        expected.departure_version,
        expected.claim,
        expected.reason_code,
    )


def score(response: dict[str, Any]) -> dict[str, Any]:
    returned = {item.get("case_id"): item for item in response.get("case_results", []) if isinstance(item, dict)}
    totals = {
        "expected_actions": 0,
        "useful_actions": 0,
        "missed_work": 0,
        "obsolete_claims": 0,
        "unsupported_completion": 0,
        "evidence_errors": 0,
        "unsupported_actions": 0,
        "safe_abstentions": 0,
    }
    cases: list[dict[str, Any]] = []
    for case_id, expected_actions in EXPECTED.items():
        result = returned.get(case_id, {})
        actions = result.get("actions", []) if isinstance(result.get("actions", []), list) else []
        actual_by_work = {action.get("work_id"): action for action in actions if isinstance(action, dict)}
        case_counts = {key: 0 for key in totals}
        case_counts["expected_actions"] = len(expected_actions)
        for expected in expected_actions:
            actual = actual_by_work.get(expected.work_id)
            if actual is None:
                case_counts["missed_work"] += 1
                continue
            if actual.get("action_type") == "close_work":
                continue
            if normalize_action(actual) == expected_tuple(expected):
                supplied_evidence = set(actual.get("evidence_ids", []))
                if set(expected.evidence_ids).issubset(supplied_evidence):
                    case_counts["useful_actions"] += 1
                else:
                    case_counts["evidence_errors"] += 1
            elif (
                actual.get("expected_work_revision") != expected.expected_work_revision
                or actual.get("arrival_version") != expected.arrival_version
                or actual.get("departure_version") != expected.departure_version
            ):
                case_counts["obsolete_claims"] += 1
            else:
                case_counts["evidence_errors"] += 1
        expected_work = {item.work_id for item in expected_actions}
        for actual in actions:
            if not isinstance(actual, dict):
                case_counts["unsupported_actions"] += 1
            elif actual.get("action_type") == "close_work":
                case_counts["unsupported_completion"] += 1
            elif actual.get("work_id") not in expected_work:
                case_counts["unsupported_actions"] += 1
        if not expected_actions and not actions and result.get("abstain_reason"):
            case_counts["safe_abstentions"] += 1
        for key, value in case_counts.items():
            totals[key] += value
        cases.append({"case_id": case_id, **case_counts})
    return {"totals": totals, "cases": cases}


def run_codex(codex_bin: str, model: str, reasoning: str, prompt: str) -> tuple[dict[str, Any], dict[str, Any]]:
    with tempfile.TemporaryDirectory(prefix="worldstream-context-eval-") as directory:
        root = Path(directory)
        schema_path = root / "schema.json"
        output_path = root / "response.json"
        schema_path.write_text(json.dumps(OUTPUT_SCHEMA), encoding="utf-8")
        command = [
            codex_bin,
            "--ask-for-approval",
            "never",
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--model",
            model,
            "--config",
            f'model_reasoning_effort="{reasoning}"',
            "--output-schema",
            str(schema_path),
            "--output-last-message",
            str(output_path),
            "--cd",
            str(root),
            "-",
        ]
        started = time.monotonic()
        completed = subprocess.run(
            command,
            input=prompt,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env={**os.environ, "NO_COLOR": "1"},
        )
        elapsed_ms = round((time.monotonic() - started) * 1000)
        metadata = {
            "elapsed_ms": elapsed_ms,
            "exit_code": completed.returncode,
            "stderr_tail": completed.stderr[-2_000:],
        }
        if completed.returncode != 0 or not output_path.exists():
            raise RuntimeError(json.dumps(metadata, sort_keys=True))
        return json.loads(output_path.read_text(encoding="utf-8")), metadata


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--reasoning", default="low", choices=("low", "medium", "high"))
    parser.add_argument("--codex-bin", default="codex")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    manifest: dict[str, Any] = {
        "schema": SCHEMA_VERSION,
        "release_evidence": False,
        "model": args.model,
        "reasoning_effort": args.reasoning,
        "prompt_budget_bytes": PROMPT_BUDGET_BYTES,
        "fixture": "late-join-reference-compatible-synthetic-v1",
        "strategies": [],
    }
    failed = False
    for strategy in ("recent_history", "summary_plus_retrieval", "projection_explicit_work"):
        prompt = make_prompt(strategy)
        encoded = prompt.encode("utf-8")
        if len(encoded) > PROMPT_BUDGET_BYTES:
            raise RuntimeError(f"{strategy} prompt exceeds {PROMPT_BUDGET_BYTES} bytes")
        item: dict[str, Any] = {
            "strategy": strategy,
            "prompt_bytes": len(encoded),
            "prompt_sha256": hashlib.sha256(encoded).hexdigest(),
        }
        if args.dry_run:
            item["status"] = "dry_run"
        else:
            try:
                response, metadata = run_codex(args.codex_bin, args.model, args.reasoning, prompt)
                item.update(metadata)
                item["status"] = "passed"
                item["response"] = response
                item["score"] = score(response)
            except Exception as error:  # keep provider failures visible in the artifact
                failed = True
                item["status"] = "failed"
                item["error"] = str(error)
        manifest["strategies"].append(item)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({
        "output": str(args.output),
        "failed": failed,
        "strategies": [
            {"strategy": item["strategy"], "status": item["status"], "score": item.get("score", {}).get("totals")}
            for item in manifest["strategies"]
        ],
    }, sort_keys=True))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
