"""Closed parsing and redaction helpers for the autonomous Hanoi Participant demo.

The helpers validate action shapes and authorized projections. They never choose
a move, infer completion from the board, or turn a Participant observation into
a policy decision.
"""

from __future__ import annotations

import json
import re
from collections.abc import Iterable
from typing import Any

PACK_ID = "worldstream.tower-of-hanoi"
PACK_VERSION = "0.1.0"
MOVE_ACTION = "move_disk"
POST_CLAIM_ACTION = "post_completion_claim"
ASSESS_CLAIM_ACTION = "assess_claim"
ACTIONS = frozenset((MOVE_ACTION, POST_CLAIM_ACTION, ASSESS_CLAIM_ACTION))
MAX_DISKS = 10
MAX_MOVE_LIMIT = 10_000
RODS = frozenset(("A", "B", "C"))
_BEARER = re.compile(r"(?i)wsb1:[0-9a-f]{64}")
_CODE = re.compile(r"[a-z0-9_]{1,80}\Z")


class HanoiProtocolError(ValueError):
    """Closed error whose message is suitable for retained local evidence."""


def redact_text(value: str) -> str:
    return _BEARER.sub("<redacted-capability>", value)


def move_from_json(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != {"from", "to", "disk"}:
        raise HanoiProtocolError("move_shape_invalid")
    source, destination, disk = value["from"], value["to"], value["disk"]
    if source not in RODS or destination not in RODS or source == destination:
        raise HanoiProtocolError("move_rods_invalid")
    if (
        isinstance(disk, bool)
        or not isinstance(disk, int)
        or not 1 <= disk <= MAX_DISKS
    ):
        raise HanoiProtocolError("move_disk_invalid")
    return {"from": source, "to": destination, "disk": disk}


def action_payload_from_json(action_type: object, value: object) -> dict[str, object]:
    """Validate one Pack Action payload without deciding whether it is appropriate."""
    if action_type == MOVE_ACTION:
        return move_from_json(value)
    if action_type == POST_CLAIM_ACTION:
        if not isinstance(value, dict) or set(value) != {"work_revision"}:
            raise HanoiProtocolError("claim_shape_invalid")
        work_revision = value["work_revision"]
        if (
            isinstance(work_revision, bool)
            or not isinstance(work_revision, int)
            or not 0 <= work_revision <= MAX_MOVE_LIMIT
        ):
            raise HanoiProtocolError("claim_revision_invalid")
        return {"work_revision": work_revision}
    if action_type == ASSESS_CLAIM_ACTION:
        if not isinstance(value, dict) or set(value) != {
            "work_revision",
            "claim_round",
            "assessment",
        }:
            raise HanoiProtocolError("assessment_shape_invalid")
        work_revision, claim_round, assessment = (
            value["work_revision"],
            value["claim_round"],
            value["assessment"],
        )
        if (
            isinstance(work_revision, bool)
            or not isinstance(work_revision, int)
            or not 0 <= work_revision <= MAX_MOVE_LIMIT
            or isinstance(claim_round, bool)
            or not isinstance(claim_round, int)
            or claim_round < 1
            or assessment not in {"endorse", "challenge", "defer"}
        ):
            raise HanoiProtocolError("assessment_invalid")
        return {
            "work_revision": work_revision,
            "claim_round": claim_round,
            "assessment": assessment,
        }
    raise HanoiProtocolError("action_type_invalid")


def action_is_offered(value: object, action_type: str) -> bool:
    if not isinstance(value, dict):
        return False
    body = value.get("projection", value)
    offers = body.get("action_offers") if isinstance(body, dict) else None
    if not isinstance(offers, list):
        return False
    return any(
        offer == action_type
        or isinstance(offer, dict)
        and offer.get("action_type") == action_type
        for offer in offers
    )


def offered_actions(value: object) -> list[str]:
    if not isinstance(value, dict):
        raise HanoiProtocolError("action_offers_invalid")
    body = value.get("projection", value)
    offers = body.get("action_offers") if isinstance(body, dict) else None
    if not isinstance(offers, list):
        raise HanoiProtocolError("action_offers_invalid")
    parsed: list[str] = []
    for offer in offers:
        action_type = (
            offer
            if isinstance(offer, str)
            else offer.get("action_type")
            if isinstance(offer, dict)
            else None
        )
        if action_type not in ACTIONS or action_type in parsed:
            raise HanoiProtocolError("action_offers_invalid")
        parsed.append(action_type)
    return parsed


def activity_from_projection(value: object) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise HanoiProtocolError("projection_invalid")
    body = value.get("projection", value)
    if not isinstance(body, dict):
        raise HanoiProtocolError("projection_invalid")
    activity = body.get("activity", body)
    if not isinstance(activity, dict):
        raise HanoiProtocolError("projection_invalid")
    return activity


def room_seq_from_projection(value: object) -> int:
    if not isinstance(value, dict):
        raise HanoiProtocolError("room_head_invalid")
    head = value.get("room_head")
    if not isinstance(head, dict):
        raise HanoiProtocolError("room_head_invalid")
    sequence = head.get("room_seq")
    if isinstance(sequence, bool) or not isinstance(sequence, int) or sequence < 0:
        raise HanoiProtocolError("room_head_invalid")
    return sequence


def _board(value: object, disks: int) -> dict[str, list[int]]:
    if not isinstance(value, dict) or set(value) != RODS:
        raise HanoiProtocolError("board_invalid")
    safe_board: dict[str, list[int]] = {}
    seen: set[int] = set()
    for rod in sorted(RODS):
        stack = value[rod]
        if not isinstance(stack, list) or any(
            isinstance(disk, bool)
            or not isinstance(disk, int)
            or not 1 <= disk <= disks
            for disk in stack
        ):
            raise HanoiProtocolError("board_invalid")
        if any(stack[index] <= stack[index + 1] for index in range(len(stack) - 1)):
            raise HanoiProtocolError("board_invalid")
        if seen.intersection(stack):
            raise HanoiProtocolError("board_invalid")
        seen.update(stack)
        safe_board[rod] = list(stack)
    if seen != set(range(1, disks + 1)):
        raise HanoiProtocolError("board_invalid")
    return safe_board


def safe_activity(
    activity: object, *, reject_answer_leak: bool = True
) -> dict[str, object]:
    """Select bounded Pack-owned facts a Participant may reason from."""
    if not isinstance(activity, dict):
        raise HanoiProtocolError("projection_invalid")
    if reject_answer_leak and any(
        key in activity
        for key in ("expected_move", "next_move", "optimal_move", "solution")
    ):
        raise HanoiProtocolError("answer_leak_projection")
    disks = activity.get("disks")
    if (
        isinstance(disks, bool)
        or not isinstance(disks, int)
        or not 1 <= disks <= MAX_DISKS
    ):
        raise HanoiProtocolError("board_invalid")
    objective = activity.get("objective")
    if (
        not isinstance(objective, dict)
        or set(objective) != {"source_rod", "target_rod", "description"}
        or objective.get("source_rod") != "A"
        or objective.get("target_rod") != "C"
        or not isinstance(objective.get("description"), str)
        or not objective["description"]
    ):
        raise HanoiProtocolError("objective_invalid")
    outcome = activity.get("outcome")
    if not isinstance(outcome, dict) or set(outcome) != {"moves", "status"}:
        raise HanoiProtocolError("outcome_invalid")
    moves, status = outcome["moves"], outcome["status"]
    if (
        isinstance(moves, bool)
        or not isinstance(moves, int)
        or not 0 <= moves <= MAX_MOVE_LIMIT
        or status not in {"in_progress", "participant_accepted_completion"}
    ):
        raise HanoiProtocolError("outcome_invalid")
    phase = activity.get("phase")
    if phase not in {"solving", "complete"}:
        raise HanoiProtocolError("phase_invalid")
    round_value = activity.get("round")
    work_revision = activity.get("work_revision")
    if (
        isinstance(round_value, bool)
        or not isinstance(round_value, int)
        or round_value < 1
        or isinstance(work_revision, bool)
        or not isinstance(work_revision, int)
        or not 0 <= work_revision <= MAX_MOVE_LIMIT
    ):
        raise HanoiProtocolError("revision_invalid")
    completion = activity.get("completion")
    if not isinstance(completion, dict):
        raise HanoiProtocolError("completion_invalid")
    required = {
        "claim_open",
        "assessments_by_member",
        "endorsement_count",
        "approval_count",
        "quorum",
    }
    if not required.issubset(completion) or set(completion) - required - {"claim"}:
        raise HanoiProtocolError("completion_invalid")
    claim_open = completion["claim_open"]
    assessments = completion["assessments_by_member"]
    endorsement_count, approval_count, quorum = (
        completion["endorsement_count"],
        completion["approval_count"],
        completion["quorum"],
    )
    if (
        not isinstance(claim_open, bool)
        or not isinstance(assessments, dict)
        or len(assessments) > 16
        or isinstance(endorsement_count, bool)
        or not isinstance(endorsement_count, int)
        or isinstance(approval_count, bool)
        or not isinstance(approval_count, int)
        or isinstance(quorum, bool)
        or not isinstance(quorum, int)
        or not 1 <= quorum <= 16
        or not 0 <= endorsement_count <= 15
        or not 0 <= approval_count <= quorum
    ):
        raise HanoiProtocolError("completion_invalid")
    safe_assessments: dict[str, str] = {}
    for member_id, assessment in assessments.items():
        if (
            not isinstance(member_id, str)
            or not member_id
            or assessment not in {"endorse", "challenge", "defer"}
        ):
            raise HanoiProtocolError("completion_invalid")
        safe_assessments[member_id] = assessment
    if (
        sum(value == "endorse" for value in safe_assessments.values())
        != endorsement_count
    ):
        raise HanoiProtocolError("completion_invalid")
    claim = completion.get("claim")
    if claim_open:
        if not isinstance(claim, dict) or set(claim) != {
            "claimant_member_id",
            "claim_round",
            "work_revision",
            "electorate_size",
            "quorum",
        }:
            raise HanoiProtocolError("completion_invalid")
        if (
            not isinstance(claim["claimant_member_id"], str)
            or not claim["claimant_member_id"]
            or isinstance(claim["claim_round"], bool)
            or not isinstance(claim["claim_round"], int)
            or claim["claim_round"] < 1
            or claim["work_revision"] != work_revision
            or isinstance(claim["electorate_size"], bool)
            or not isinstance(claim["electorate_size"], int)
            or not 1 <= claim["electorate_size"] <= 16
            or claim["quorum"] != quorum
        ):
            raise HanoiProtocolError("completion_invalid")
        safe_claim: dict[str, object] | None = dict(claim)
    elif claim is not None or safe_assessments or endorsement_count or approval_count:
        raise HanoiProtocolError("completion_invalid")
    else:
        safe_claim = None
    return {
        "board": _board(activity.get("board"), disks),
        "disks": disks,
        "objective": {
            "source_rod": "A",
            "target_rod": "C",
            "description": objective["description"],
        },
        "outcome": {"moves": moves, "status": status},
        "phase": phase,
        "round": round_value,
        "work_revision": work_revision,
        "completion": {
            "claim_open": claim_open,
            **({"claim": safe_claim} if safe_claim is not None else {}),
            "assessments_by_member": safe_assessments,
            "endorsement_count": endorsement_count,
            "approval_count": approval_count,
            "quorum": quorum,
        },
    }


def closed_code(value: object, fallback: str = "action_not_accepted") -> str:
    return value if isinstance(value, str) and _CODE.fullmatch(value) else fallback


def json_results_in(lines: Iterable[str]) -> list[dict[str, object]]:
    """Extract bounded command JSON objects from a Codex JSONL transcript."""
    values: list[dict[str, object]] = []
    for line in lines:
        if len(line.encode("utf-8")) > 1_048_576:
            raise HanoiProtocolError("solver_jsonl_invalid")
        try:
            envelope = json.loads(line)
        except json.JSONDecodeError as error:
            raise HanoiProtocolError("solver_jsonl_invalid") from error
        values.extend(_json_results_in(envelope))
    return values


def _json_results_in(value: object) -> list[dict[str, object]]:
    if isinstance(value, str):
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return []
        return [parsed] if isinstance(parsed, dict) else []
    if isinstance(value, list):
        return [result for item in value for result in _json_results_in(item)]
    if isinstance(value, dict):
        return [result for item in value.values() for result in _json_results_in(item)]
    return []
