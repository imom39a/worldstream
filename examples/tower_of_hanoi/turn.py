"""Read one safe Hanoi view or submit one Participant-owned Pack Action."""

from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
from typing import Any

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    sdk_base_url,
)

from .protocol import (
    ACTIONS,
    PACK_ID,
    HanoiProtocolError,
    action_is_offered,
    action_payload_from_json,
    activity_from_projection,
    closed_code,
    offered_actions,
    room_seq_from_projection,
    safe_activity,
)


def ready_snapshot_in_jsonl(lines: list[str]) -> bool:
    """Confirm that an autonomous invocation obtained one bounded current view."""
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        item = event.get("item") if isinstance(event, dict) else None
        if not isinstance(item, dict) or item.get("type") != "command_execution":
            continue
        command, output = item.get("command"), item.get("aggregated_output")
        if (
            not isinstance(command, str)
            or "examples.tower_of_hanoi.turn" not in command
            or " snapshot " not in command
            or item.get("status") != "completed"
            or item.get("exit_code") != 0
            or not isinstance(output, str)
            or len(output.encode("utf-8")) > 32_768
        ):
            continue
        try:
            value = json.loads(output)
            if not isinstance(value, dict) or value.get("status") != "ready":
                continue
            safe_activity(value)
            room_seq_from_projection({"room_head": {"room_seq": value["room_seq"]}})
        except (HanoiProtocolError, KeyError, json.JSONDecodeError):
            continue
        return True
    return False


def _membership(path: Path, required_role: str) -> dict[str, Any]:
    document = load_membership(path)
    pack = document["pack"]
    if pack["id"] != PACK_ID or document["role"] != required_role:
        raise CredentialError("tower_membership_role_required")
    return document


def _attachment_metadata(room: Any, synchronized: object) -> dict[str, object]:
    attached = room.attached if isinstance(room.attached, dict) else {}
    head = attached.get("room_head")
    room_seq = head.get("room_seq") if isinstance(head, dict) else None
    reset = (
        room.last_projection_reset
        if isinstance(room.last_projection_reset, dict)
        else None
    )
    return {
        "attached": {
            "access_mode": attached.get("access_mode"),
            "membership_status": attached.get("membership_status"),
            "role": attached.get("role"),
            "room_status": attached.get("room_status"),
            "room_seq": room_seq,
        },
        "sync": {
            "message_count": len(synchronized) if isinstance(synchronized, list) else 0,
            "projection_reset_observed": reset is not None,
            "cursor": room.cursor,
        },
    }


def _safe_receipt(receipt: object) -> dict[str, object]:
    if not isinstance(receipt, dict):
        return {"kind": "invalid"}
    head = receipt.get("room_head")
    room_seq = head.get("room_seq") if isinstance(head, dict) else None
    return {
        "action_id": receipt.get("action_id")
        if isinstance(receipt.get("action_id"), str)
        else None,
        "code": closed_code(receipt.get("code"))
        if receipt.get("code") is not None
        else None,
        "current_room_seq": receipt.get("current_room_seq")
        if isinstance(receipt.get("current_room_seq"), int)
        else None,
        "duplicate": receipt.get("duplicate")
        if isinstance(receipt.get("duplicate"), bool)
        else None,
        "room_seq": room_seq if isinstance(room_seq, int) else None,
        "transition_id": receipt.get("transition_id")
        if isinstance(receipt.get("transition_id"), str)
        else None,
    }


async def snapshot(path: Path, role: str = "solver") -> dict[str, object]:
    """Attach with one Membership and return only safe Projection facts."""
    from worldstream_sdk import Client

    membership = _membership(path, role)
    client = Client(sdk_base_url(membership), membership["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    try:
        synchronized = await room.sync()
        projection = await client.projection(membership["room_id"])
        activity = safe_activity(activity_from_projection(projection))
        actions = offered_actions(projection) if role == "solver" else []
        return {
            "schema": "worldstream/tower-of-hanoi-participant-view/v2",
            "status": "ready",
            "room_seq": room_seq_from_projection(projection),
            "action_offers": actions,
            "rules": [
                "move only the top disk of a rod",
                "never place a larger disk on a smaller disk",
                "a move changes work_revision and supersedes an open completion claim",
                "a completion claim is participant evidence, not a Pack verdict about the board",
            ],
            "transport": _attachment_metadata(room, synchronized),
            **activity,
        }
    finally:
        await room.close()


async def act(
    path: Path,
    action_type: str,
    payload: object,
    expected_room_seq: int,
    timeout: float,
) -> dict[str, object]:
    """Submit exactly one self-selected offered action against a supplied Head."""
    from worldstream_sdk import Client, LostActionReply

    if action_type not in ACTIONS:
        raise HanoiProtocolError("action_type_invalid")
    if (
        isinstance(expected_room_seq, bool)
        or not isinstance(expected_room_seq, int)
        or expected_room_seq < 0
    ):
        raise HanoiProtocolError("room_head_invalid")
    safe_payload = action_payload_from_json(action_type, payload)
    membership = _membership(path, "solver")
    client = Client(sdk_base_url(membership), membership["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    try:
        synchronized = await room.sync()
        current = await client.projection(membership["room_id"])
        current_room_seq = room_seq_from_projection(current)
        if current_room_seq != expected_room_seq:
            return {
                "status": "stale",
                "code": "stale_head",
                "current_room_seq": current_room_seq,
                "action_type": action_type,
                "transport": _attachment_metadata(room, synchronized),
            }
        if not action_is_offered(current, action_type):
            return {
                "status": "rejected",
                "code": "action_not_offered",
                "current_room_seq": current_room_seq,
                "action_type": action_type,
                "transport": _attachment_metadata(room, synchronized),
            }
        try:
            receipt = await room.act(
                action_type,
                safe_payload,
                expected_room_seq=expected_room_seq,
                timeout=timeout,
            )
        except LostActionReply as error:
            receipt = await error.retry(timeout=timeout)
        head = receipt.get("room_head")
        sequence = head.get("room_seq") if isinstance(head, dict) else None
        if isinstance(sequence, bool) or not isinstance(sequence, int) or sequence < 0:
            return {
                "status": "stale"
                if closed_code(receipt.get("code"))
                in {"stale_head", "stale_room_state"}
                else "rejected",
                "code": closed_code(receipt.get("code")),
                "current_room_seq": current_room_seq,
                "action_type": action_type,
                "transport": _attachment_metadata(room, synchronized),
                "action_receipt": _safe_receipt(receipt),
            }
        updated = await client.projection(membership["room_id"])
        activity = safe_activity(activity_from_projection(updated))
        return {
            "status": "accepted",
            "room_seq": sequence,
            "action_type": action_type,
            "phase": activity["phase"],
            "outcome": activity["outcome"],
            "work_revision": activity["work_revision"],
            "transport": _attachment_metadata(room, synchronized),
            "action_receipt": _safe_receipt(receipt),
        }
    finally:
        await room.close()


async def verify_terminal(path: Path, role: str = "observer") -> dict[str, object]:
    from worldstream_sdk import Client

    membership = _membership(path, role)
    client = Client(sdk_base_url(membership), membership["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    try:
        synchronized = await room.sync()
        current = await client.projection(membership["room_id"])
        room_seq = room_seq_from_projection(current)
        current_activity = safe_activity(activity_from_projection(current))
        if current_activity["outcome"]["status"] != "participant_accepted_completion":
            raise HanoiProtocolError("terminal_outcome_missing")
        replay = await client.replay(membership["room_id"], room_seq)
        replay_activity = safe_activity(activity_from_projection(replay))
        replay_head = room_seq_from_projection(replay)
        if replay.get("requested_room_seq") != room_seq or replay_head != room_seq:
            raise HanoiProtocolError("replay_head_mismatch")
        if current_activity != replay_activity:
            raise HanoiProtocolError("replay_projection_mismatch")
        return {
            "status": "verified",
            "complete_head_room_seq": room_seq,
            "current_projection": current_activity,
            "replay_equivalent": True,
            "transport": _attachment_metadata(room, synchronized),
        }
    finally:
        await room.close()


async def verify_genesis_replay(
    path: Path, role: str = "observer"
) -> dict[str, object]:
    from worldstream_sdk import Client

    membership = _membership(path, role)
    client = Client(sdk_base_url(membership), membership["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    try:
        synchronized = await room.sync()
        current = await client.projection(membership["room_id"])
        if room_seq_from_projection(current) != 0:
            raise HanoiProtocolError("genesis_replay_head_invalid")
        current_activity = safe_activity(activity_from_projection(current))
        replay = await client.replay(membership["room_id"], 0)
        replay_activity = safe_activity(activity_from_projection(replay))
        if (
            replay.get("requested_room_seq") != 0
            or room_seq_from_projection(replay) != 0
        ):
            raise HanoiProtocolError("genesis_replay_head_invalid")
        if current_activity != replay_activity:
            raise HanoiProtocolError("genesis_replay_projection_mismatch")
        return {
            "status": "verified",
            "room_seq": 0,
            "current_projection": current_activity,
            "replay_equivalent": True,
            "transport": _attachment_metadata(room, synchronized),
        }
    finally:
        await room.close()


def _arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    for name in ("snapshot", "observe"):
        command = subcommands.add_parser(name)
        command.add_argument("--membership-file", required=True, type=Path)
        command.add_argument(
            "--role", default="solver" if name == "snapshot" else "observer"
        )
    action = subcommands.add_parser("act")
    action.add_argument("--membership-file", required=True, type=Path)
    action.add_argument("--room-seq", required=True, type=int)
    action.add_argument("--action-type", required=True, choices=sorted(ACTIONS))
    action.add_argument("--payload-json", required=True)
    action.add_argument("--timeout-seconds", type=float, default=15.0)
    terminal = subcommands.add_parser("verify-terminal")
    terminal.add_argument("--membership-file", required=True, type=Path)
    terminal.add_argument("--role", default="observer")
    return parser.parse_args()


def main() -> int:
    arguments = _arguments()
    try:
        if arguments.command in {"snapshot", "observe"}:
            result = asyncio.run(snapshot(arguments.membership_file, arguments.role))
        elif arguments.command == "act":
            if not 1 <= arguments.timeout_seconds <= 60:
                raise HanoiProtocolError("timeout_invalid")
            result = asyncio.run(
                act(
                    arguments.membership_file,
                    arguments.action_type,
                    json.loads(arguments.payload_json),
                    arguments.room_seq,
                    arguments.timeout_seconds,
                )
            )
        else:
            result = asyncio.run(
                verify_terminal(arguments.membership_file, arguments.role)
            )
    except (
        CredentialError,
        HanoiProtocolError,
        OSError,
        TimeoutError,
        ValueError,
    ) as error:
        result = {"status": "error", "code": type(error).__name__}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return (
        0
        if result.get("status")
        in {"ready", "accepted", "verified", "stale", "rejected"}
        else 3
    )


if __name__ == "__main__":
    raise SystemExit(main())
