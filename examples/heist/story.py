"""Offline, deterministic IMO-57 Agent Heist evidence story.

This module is deliberately a small protocol/story harness.  It does not
connect to WorldStream, SQLite, PostgreSQL, a browser, or a model provider.
The reducer-shaped state machine mirrors the public Heist rules needed by the
absent-Broker path; operational activation evidence is kept separate from
canonical transitions.
"""

from __future__ import annotations

import copy
import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

HASH_ALGORITHM = "sha256"
PACK_ID = "worldstream.agent-heist"
PACK_VERSION = "0.1.0"
# This is the exact selectable semantic revision used by the public
# disposable-daemon Heist runs. The offline story exercises retained replay
# with that registered revision; it does not invent a separate executor.
PACK_DIGEST = "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407"
ROOM_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC5"
CREATED_AT = "2026-08-15T12:00:00Z"
FIXTURE_ID = "service_window"
FIXTURE = {
    "fixture_id": FIXTURE_ID,
    "route": "service",
    "entry_window": "early",
    "required_tool": "thermal_key",
    "extraction": "boat",
}
PLAN_ID = "01ARZ3NDEKTSV4RRFFQ69G5FNP"
ROLES = ("navigator", "insider", "broker")
MEMBERS = {
    "navigator": "01ARZ3NDEKTSV4RRFFQ69G5FC0",
    "insider": "01ARZ3NDEKTSV4RRFFQ69G5FC1",
    "broker": "01ARZ3NDEKTSV4RRFFQ69G5FC2",
}
PRINCIPALS = {
    "navigator": "01ARZ3NDEKTSV4RRFFQ69G5FD0",
    "insider": "01ARZ3NDEKTSV4RRFFQ69G5FD1",
    "broker": "01ARZ3NDEKTSV4RRFFQ69G5FD2",
}
CLUES = {
    "navigator": ("route",),
    "insider": ("entry_window",),
    "broker": ("required_tool", "extraction"),
}
ACTION_ORDER = (
    "inspect_clue",
    "publish_clue",
    "offer_exchange",
    "accept_exchange",
    "propose_plan",
    "endorse_plan",
    "challenge_plan",
    "commit_move",
    "acknowledge_result",
)
MAX_OPEN_OFFERS_PER_ROLE = 4
ATTENTION_PRECEDENCE = (
    "required_action_deadline",
    "commitment_opened",
    "offer_received",
    "endorsement_requested",
    "round_result_available",
)

PARITY_FIXTURE = "parity_fixture.json"
MAX_TRANSCRIPT_BYTES = 512 * 1024
MAX_SAFE_INTEGER = 9_007_199_254_740_991
_ULID_RE = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}\Z")


def canonical_bytes(value: Any) -> bytes:
    """Encode the JSON subset used by the evidence corpus canonically."""

    _validate_canonical_value(value)
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def _validate_canonical_value(value: Any, label: str = "value") -> None:
    if value is None or isinstance(value, (bool, str)):
        return
    if type(value) is int:
        if not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
            raise ValueError(
                f"{label} contains an integer outside the canonical safe range"
            )
        return
    if isinstance(value, float):
        raise TypeError(f"{label} contains a floating-point value")
    if isinstance(value, list):
        for index, item in enumerate(value):
            _validate_canonical_value(item, f"{label}[{index}]")
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError(f"{label} contains a non-string object key")
            _validate_canonical_value(item, f"{label}.{key}")
        return
    raise ValueError(f"{label} contains a value outside the canonical JSON subset")


def canonical_text(value: Any) -> str:
    return canonical_bytes(value).decode("utf-8")


def digest(value: Any) -> str:
    return f"{HASH_ALGORITHM}:{hashlib.sha256(canonical_bytes(value)).hexdigest()}"


def _require_ulid(value: Any, label: str) -> None:
    if not isinstance(value, str) or _ULID_RE.fullmatch(value) is None:
        raise ValueError(f"{label} must be a canonical ULID")


def validate_transcript(transcript: dict[str, Any]) -> None:
    """Fail closed on malformed offline evidence before it is printed."""

    expected_fields = {
        "schema",
        "description",
        "hash_algorithm",
        "canonical_json",
        "reference",
        "fixture",
        "roles",
        "phase_path",
        "genesis",
        "transitions",
        "operational_evidence",
        "expected_fixture_outcome",
        "final_read_only_replay",
        "final_head",
        "transcript_digest",
    }
    if not isinstance(transcript, dict) or set(transcript) != expected_fields:
        raise ValueError("transcript does not match the offline evidence schema")
    if transcript["schema"] != "worldstream/imo-57-absent-broker-story/v1":
        raise ValueError("unsupported transcript schema")
    if transcript["hash_algorithm"] != HASH_ALGORITHM:
        raise ValueError("unsupported transcript hash algorithm")
    if transcript["canonical_json"] != "sorted UTF-8 JSON, no insignificant whitespace":
        raise ValueError("unsupported transcript canonicalization")
    if len(canonical_bytes(transcript)) > MAX_TRANSCRIPT_BYTES:
        raise ValueError("transcript exceeds the configured bound")
    if transcript["reference"] != {"pack_id": PACK_ID, "pack_version": PACK_VERSION}:
        raise ValueError("transcript pack reference is inconsistent")
    if transcript["fixture"] != FIXTURE:
        raise ValueError("transcript fixture is inconsistent")
    if transcript["roles"] != {
        "navigator": "cooperative",
        "insider": "cautious",
        "broker": "withholding/absent runner; immutable seat remains present",
    }:
        raise ValueError("transcript role strategy is inconsistent")

    genesis = transcript["genesis"]
    final_head = transcript["final_head"]
    if not isinstance(genesis, dict) or not isinstance(final_head, dict):
        raise TypeError("transcript heads must be objects")
    _require_ulid(genesis.get("room_id"), "genesis.room_id")
    _require_ulid(final_head.get("room_id"), "final_head.room_id")
    if genesis["room_id"] != final_head["room_id"] or genesis["room_id"] != ROOM_ID:
        raise ValueError("transcript room identity is inconsistent")
    transitions = transcript["transitions"]
    if not isinstance(transitions, list) or len(transitions) > 1024:
        raise ValueError("transitions are outside the configured bound")
    expected_initial = _initial_state()
    if (
        not isinstance(genesis.get("initial_state"), dict)
        or genesis["initial_state"] != expected_initial
    ):
        raise ValueError("genesis initial state is inconsistent")
    if set(genesis) != {
        "kind",
        "room_id",
        "pack_id",
        "pack_version",
        "created_at",
        "configuration",
        "fixture",
        "initial_state",
        "initial_timer",
        "genesis_hash",
    }:
        raise ValueError("genesis does not match the offline evidence schema")
    genesis_without_hash = {
        key: value for key, value in genesis.items() if key != "genesis_hash"
    }
    if genesis["genesis_hash"] != digest(genesis_without_hash):
        raise ValueError("genesis hash does not match canonical evidence")

    replay = transcript["final_read_only_replay"]
    if (
        not isinstance(replay, dict)
        or set(replay)
        != {
            "read_only",
            "source",
            "snapshots_used",
            "activation_created",
            "current_head",
            "replayed_activity_state_hash",
            "verified_transition_count",
            "verified",
        }
        or replay.get("read_only") is not True
    ):
        raise ValueError("replay must remain read-only")
    if replay.get("verified") is not True:
        raise ValueError("replay verification is required")

    for role, member_id in MEMBERS.items():
        _require_ulid(member_id, f"members.{role}")
        _require_ulid(PRINCIPALS[role], f"principals.{role}")
    for evidence in transcript["operational_evidence"]:
        if not isinstance(evidence, dict):
            raise TypeError("operational evidence record is invalid")
        if evidence.get("canonical") is not False:
            raise ValueError(
                "operational evidence must remain outside the canonical chain"
            )
        if evidence.get("kind") == "fresh_activation_context":
            context = evidence.get("context")
            if not isinstance(context, dict):
                raise ValueError("activation context is invalid")
            witness = context.get("membership_witness")
            if not isinstance(witness, dict):
                raise ValueError("activation membership witness is missing")
            _require_ulid(witness.get("member_id"), "membership_witness.member_id")
            _require_ulid(
                witness.get("principal_id"), "membership_witness.principal_id"
            )
            if evidence.get("participant_action_authority") != "not_granted_by_claim":
                raise ValueError(
                    "activation claim must not grant participant authority"
                )
            lease_witness = context.get("lease_witness")
            if lease_witness != {
                "activation_id": evidence.get("activation_id"),
                "lease_generation": 0,
                "fencing": "compare_and_swap_generation",
            }:
                raise ValueError("activation lease witness is invalid")
        if (
            evidence.get("kind") == "participant_authority"
            and evidence.get("authority") != "participant-action-capability"
        ):
            raise ValueError("participant authority evidence is invalid")

    replayed_state, replayed_head, activation_created, replayed_transition_hashes = (
        _replay_records(
            genesis["genesis_hash"],
            genesis["initial_state"],
            transitions,
            _core_state(),
        )
    )
    expected_phase_path = [expected_initial["phase"]] + [
        transition["state_after"]["phase"] for transition in transitions
    ]
    if transcript["phase_path"] != expected_phase_path:
        raise ValueError("phase path does not match the replayed reducer")
    if final_head != replayed_head:
        raise ValueError("final head does not match the replayed reducer")
    comparison = _replay_hash_comparison(
        final_head,
        replayed_head,
        [transition["transition_hash"] for transition in transitions],
        replayed_transition_hashes,
    )
    if not all(item["match"] for item in comparison.values()):
        raise ValueError("replay did not match every retained canonical hash")
    if transcript["expected_fixture_outcome"] != _outcome(replayed_state):
        raise ValueError("fixture outcome does not match the replayed reducer")
    if replay["current_head"] != replayed_head:
        raise ValueError("replay head is inconsistent")
    if replay["replayed_activity_state_hash"] != digest(replayed_state):
        raise ValueError("replay activity hash is inconsistent")
    if replay["verified_transition_count"] != len(transitions):
        raise ValueError("replay transition count is inconsistent")
    if replay["activation_created"] != activation_created:
        raise ValueError("replay activation count is inconsistent")
    if (
        replay["source"] != "genesis_plus_ordered_transitions"
        or replay["snapshots_used"] != 0
    ):
        raise ValueError("replay source is not the deterministic offline path")

    recorded_digest = transcript["transcript_digest"]
    without_digest = {
        key: value for key, value in transcript.items() if key != "transcript_digest"
    }
    if not isinstance(recorded_digest, str) or recorded_digest != digest(
        without_digest
    ):
        raise ValueError("transcript digest does not match canonical evidence")


def _initial_state() -> dict[str, Any]:
    return {
        "phase": "Briefing",
        "phase_generation": 1,
        "phase_start": CREATED_AT,
        "phase_deadline": "2026-08-15T12:00:30Z",
        "fixture_id": FIXTURE_ID,
        "fixture": copy.deepcopy(FIXTURE),
        "known_clues": {role: [] for role in ROLES},
        "published_claims": [],
        "plans": [],
        "endorsements": {},
        "commitments": {},
        "result_acknowledgements": [],
        "outcome": None,
    }


def _core_state() -> dict[str, Any]:
    return {
        "room_status": "active",
        "memberships": {
            role: {
                "member_id": MEMBERS[role],
                "principal_id": PRINCIPALS[role],
                "principal_kind": "agent",
                "standing": "enabled",
                "access_mode": "participant",
                "role": role,
            }
            for role in ROLES
        },
    }


def _outcome(state: dict[str, Any]) -> dict[str, Any]:
    counts: dict[str, int] = {}
    for commitment in state["commitments"].values():
        plan_id = commitment["selected_plan_id"]
        counts[plan_id] = counts.get(plan_id, 0) + 1
    majority = [plan for plan, count in counts.items() if count >= 2]
    missing = [role for role in ROLES if role not in state["commitments"]]
    if len(majority) != 1:
        return {
            "outcome": "failure",
            "selected_plan_id": None,
            "vote_counts": dict(sorted(counts.items())),
            "missing_roles": missing,
            "checks": None,
            "score": 0,
            "reason": "no_strict_majority",
        }
    selected = majority[0]
    plan = next((item for item in state["plans"] if item["plan_id"] == selected), None)
    if plan is None:
        return {
            "outcome": "failure",
            "selected_plan_id": selected,
            "vote_counts": dict(sorted(counts.items())),
            "missing_roles": missing,
            "checks": None,
            "score": 0,
            "reason": "no_strict_majority",
        }
    checks = {
        "route": plan["route"] == state["fixture"]["route"],
        "entry_window": plan["entry_window"] == state["fixture"]["entry_window"],
        "required_tool": plan["required_tool"] == state["fixture"]["required_tool"],
        "extraction": plan["extraction"] == state["fixture"]["extraction"],
        "resource_contributed": any(
            item["selected_plan_id"] == selected
            and item["contribute_required_resource"]
            for item in state["commitments"].values()
        ),
    }
    score = sum(checks.values())
    return {
        "outcome": "success"
        if score == 5
        else "partial_failure"
        if score >= 3
        else "failure",
        "selected_plan_id": selected,
        "vote_counts": dict(sorted(counts.items())),
        "missing_roles": missing,
        "checks": checks,
        "score": score,
        "reason": "scored_selected_plan",
    }


def reduce_state(before: dict[str, Any], stimulus: dict[str, Any]) -> dict[str, Any]:
    """The tiny deterministic reducer used both while producing and replaying."""

    state = copy.deepcopy(before)
    if stimulus["kind"] == "action":
        action = stimulus["action_type"]
        role = stimulus["role"]
        payload = stimulus["payload"]
        if action == "inspect_clue":
            clue_id = payload["clue_id"]
            if clue_id not in state["known_clues"][role]:
                state["known_clues"][role].append(clue_id)
        elif action == "publish_clue":
            if payload["clue_id"] not in state["published_claims"]:
                state["published_claims"].append(payload["clue_id"])
        elif action == "propose_plan":
            state["plans"].append(
                {
                    "plan_id": stimulus["action_id"],
                    "proposer_role": role,
                    "created_room_seq": stimulus["next_room_seq"],
                    **payload,
                }
            )
        elif action == "endorse_plan":
            state["endorsements"][role] = payload["plan_id"]
        elif action == "commit_move":
            state["commitments"][role] = {
                "selected_plan_id": payload["selected_plan_id"],
                "contribute_required_resource": payload["contribute_required_resource"],
            }
        elif action == "acknowledge_result":
            if role not in state["result_acknowledgements"]:
                state["result_acknowledgements"].append(role)
            if len(state["result_acknowledgements"]) == 3:
                state["phase"] = "Complete"
                state["phase_generation"] += 1
                state["phase_start"] = stimulus["admitted_at"]
                state["phase_deadline"] = None
        return state

    timer = stimulus["timer"]
    kind = timer["kind"]
    state["phase_generation"] += 1
    state["phase_start"] = timer["scheduled_for"]
    if kind == "briefing_deadline":
        state["phase"] = "Negotiation"
        state["phase_deadline"] = "2026-08-15T12:02:00Z"
    elif kind == "negotiation_deadline":
        state["phase"] = "Commitment"
        state["phase_deadline"] = "2026-08-15T12:02:30Z"
    elif kind == "commitment_deadline":
        state["phase"] = "Resolution"
        state["phase_deadline"] = "2026-08-15T12:02:30.000001Z"
    elif kind == "resolve_now":
        state["phase"] = "Result"
        state["phase_deadline"] = "2026-08-15T12:02:50.000001Z"
        state["outcome"] = _outcome(state)
    elif kind == "result_deadline":
        state["phase"] = "Complete"
        state["phase_deadline"] = None
    else:
        # Reminder is a committed Stimulus but intentionally does not change
        # Activity State, matching the public reducer's reminder behavior.
        state["phase_generation"] -= 1
        state["phase_start"] = before["phase_start"]
    return state


def public_projection(state: dict[str, Any], role: str | None = None) -> dict[str, Any]:
    projection: dict[str, Any] = {
        "phase": state["phase"],
        "phase_generation": state["phase_generation"],
        "phase_start": state["phase_start"],
        "phase_deadline": state["phase_deadline"],
        "seats": [{"role": item, "present": True} for item in ROLES],
        "public_claims": [
            {"clue_id": clue, "claim_code": f"{clue}_{FIXTURE[clue]}"}
            for clue in state["published_claims"]
        ],
        "plans": copy.deepcopy(state["plans"]),
        "endorsements": copy.deepcopy(state["endorsements"]),
        "challenges": [],
        "commitment_count": len(state["commitments"]),
        "outcome": copy.deepcopy(state["outcome"]),
    }
    if role is not None:
        projection["private_clues"] = [
            {
                "clue_id": clue,
                "known": True,
                "owner_role": role,
                "claim_code": f"{clue}_{FIXTURE[clue]}",
            }
            for clue in state["known_clues"][role]
        ]
        projection["own_commitment"] = copy.deepcopy(state["commitments"].get(role))
        projection["addressed_offers"] = []
    return projection


def audience_projection(
    state: dict[str, Any], audience: str, role: str | None = None
) -> dict[str, Any]:
    """Return the deliberately bounded projection for one story audience.

    The reducer state is richer than any one consumer is authorized to see.
    Keeping these projections explicit makes privacy noninterference testable:
    private clues and commitments may change participant views without
    changing public, operator, or historical views.
    """

    if audience == "public":
        if role is not None:
            raise ValueError("public projection cannot name a participant")
        return public_projection(state)
    if audience == "participant":
        if role not in ROLES:
            raise ValueError("participant projection requires a known role")
        return public_projection(state, role)
    if audience in {"operator", "historical"}:
        projection = public_projection(state)
        if audience == "historical":
            projection.pop("outcome", None)
            projection["historical"] = True
        else:
            projection["operator"] = {"redacted": True}
        return projection
    raise ValueError("unknown projection audience")


def action_offers(state: dict[str, Any], role: str) -> list[str]:
    offers: list[str] = []
    if state["phase"] in {"Briefing", "Negotiation", "Commitment"} and any(
        clue not in state["known_clues"][role] for clue in CLUES[role]
    ):
        offers.append("inspect_clue")
    if state["phase"] == "Negotiation":
        offers.extend(
            [
                "publish_clue",
                "offer_exchange",
                "accept_exchange",
                "propose_plan",
                "endorse_plan",
                "challenge_plan",
            ]
        )
    if state["phase"] == "Commitment" and role not in state["commitments"]:
        offers.append("commit_move")
    if state["phase"] == "Result" and role not in state["result_acknowledgements"]:
        offers.append("acknowledge_result")
    return [action for action in ACTION_ORDER if action in offers][
        :MAX_OPEN_OFFERS_PER_ROLE
    ]


def attention_signal(state: dict[str, Any], role: str, reason: str) -> dict[str, Any]:
    action_types = {
        "offer_received": ["accept_exchange"],
        "endorsement_requested": ["endorse_plan", "challenge_plan"],
        "commitment_opened": ["commit_move"],
        "required_action_deadline": ["commit_move"],
        "round_result_available": ["acknowledge_result"],
    }[reason]
    return {
        "target_member_id": MEMBERS[role],
        "reason": reason,
        "priority": 1,
        "deduplication_key": f"{reason}:{role}:{state['phase_generation']}",
        "deadline": state["phase_deadline"],
        "action_types": action_types,
    }


@dataclass
class ActivationLedger:
    """Operational evidence only; nothing here is part of the hash chain."""

    records: list[dict[str, Any]]
    activations: dict[str, dict[str, Any]]
    receipts: dict[tuple[str, str], dict[str, Any]]

    @classmethod
    def create(cls) -> ActivationLedger:
        return cls([], {}, {})

    def _record(self, record: dict[str, Any]) -> dict[str, Any]:
        safe_record = {**record, "canonical": False}
        self.records.append(safe_record)
        return safe_record

    def create_from_attention(
        self,
        cause_seq: int,
        signal: dict[str, Any],
        state: dict[str, Any],
        head: dict[str, Any],
    ) -> str:
        activation_id = f"activation-{cause_seq}-{signal['deduplication_key']}"
        if activation_id in self.activations:
            self._record(
                {
                    "kind": "activation_create",
                    "outcome": "duplicate_existing",
                    "activation_id": activation_id,
                    "cause_seq": cause_seq,
                }
            )
            return activation_id
        context = {
            "head": copy.deepcopy(head),
            "projection": public_projection(state, "broker"),
            "projection_hash": digest(public_projection(state, "broker")),
            "action_offers": action_offers(state, "broker"),
            "membership_witness": copy.deepcopy(_core_state()["memberships"]["broker"]),
            "integrity_witness": {"state": "healthy", "generation": 1},
            "policy_witness": {"revision": "policy-v1", "decision": "allow"},
            "authority_witness": {
                "activation_runner": "runner-control-capability",
                "participant_action": "separate-capability-required",
            },
            "lease_witness": {
                "activation_id": activation_id,
                "lease_generation": 0,
                "fencing": "compare_and_swap_generation",
            },
            "delivery_witness": {"kind": "projection_reset", "cursor": None},
            "budget": {"max_seconds": 30},
            "deadline": signal["deadline"],
            "artifacts": [],
        }
        context_hash = digest(context)
        self.activations[activation_id] = {
            "activation_id": activation_id,
            "cause_seq": cause_seq,
            "signal": copy.deepcopy(signal),
            "status": "pending",
            "lease_generation": 0,
            "context": context,
            "context_hash": context_hash,
        }
        self._record(
            {
                "kind": "activation_create",
                "outcome": "created",
                "activation_id": activation_id,
                "cause_seq": cause_seq,
                "context_hash": context_hash,
                "canonical": False,
            }
        )
        return activation_id

    def claim(
        self,
        activation_id: str,
        runner: str,
        operation_id: str,
        request_hash: str,
        now: str,
        lease_until: str,
        expected_generation: int,
    ) -> dict[str, Any]:
        key = (operation_id, request_hash)
        if key in self.receipts:
            result = {**self.receipts[key], "outcome": "duplicate_existing"}
            self._record({"kind": "activation_claim", **result})
            return result
        activation = self.activations[activation_id]
        if activation["lease_generation"] != expected_generation:
            result = {"outcome": "stale_generation", "activation_id": activation_id}
        elif activation["status"] == "leased":
            result = {"outcome": "already_leased", "activation_id": activation_id}
        elif activation["status"] in {"pending", "expired"}:
            activation["status"] = "leased"
            activation["lease_generation"] = expected_generation + 1
            result = {
                "outcome": "claimed",
                "activation_id": activation_id,
                "runner": runner,
                "lease_generation": activation["lease_generation"],
                "lease_until": lease_until,
                "context_hash": activation["context_hash"],
                "context": copy.deepcopy(activation["context"]),
            }
        else:
            result = {"outcome": "not_claimable", "activation_id": activation_id}
        self.receipts[key] = copy.deepcopy(result)
        self._record(
            {"kind": "activation_claim", **result, "operation_id": operation_id}
        )
        return result

    def expire_and_reclaim(
        self, activation_id: str, now: str, runner: str
    ) -> dict[str, Any]:
        activation = self.activations[activation_id]
        old_generation = activation["lease_generation"]
        activation["status"] = "expired"
        self._record(
            {
                "kind": "activation_lease",
                "outcome": "expired",
                "activation_id": activation_id,
                "lease_generation": old_generation,
                "at": now,
            }
        )
        activation["status"] = "leased"
        activation["lease_generation"] += 1
        result = {
            "kind": "activation_claim",
            "outcome": "reclaimed",
            "activation_id": activation_id,
            "runner": runner,
            "lease_generation": activation["lease_generation"],
            "context_hash": activation["context_hash"],
        }
        self._record(result)
        return result

    def complete(
        self,
        activation_id: str,
        operation_id: str,
        request_hash: str,
        lease_generation: int,
        result_code: str,
    ) -> dict[str, Any]:
        key = (operation_id, request_hash)
        if key in self.receipts:
            result = {**self.receipts[key], "outcome": "idempotent_existing"}
            self._record({"kind": "activation_complete", **result})
            return result
        activation = self.activations[activation_id]
        if lease_generation != activation["lease_generation"]:
            result = {
                "outcome": "stale_generation",
                "activation_id": activation_id,
                "expected_generation": activation["lease_generation"],
                "provided_generation": lease_generation,
            }
        else:
            activation["status"] = "completed"
            result = {
                "outcome": "completed",
                "activation_id": activation_id,
                "result_code": result_code,
                "result_hash": digest(
                    {"activation_id": activation_id, "result_code": result_code}
                ),
            }
        self.receipts[key] = copy.deepcopy(result)
        self._record(
            {"kind": "activation_complete", **result, "operation_id": operation_id}
        )
        return result


class Story:
    def __init__(self) -> None:
        self.core = _core_state()
        self.state = _initial_state()
        self.transitions: list[dict[str, Any]] = []
        self.evidence: list[dict[str, Any]] = []
        self.activation = ActivationLedger.create()
        self.action_receipts: dict[str, tuple[str, dict[str, Any]]] = {}
        self.last_action_was_duplicate = False
        self.genesis_body = {
            "kind": "genesis",
            "room_id": ROOM_ID,
            "pack_id": PACK_ID,
            "pack_version": PACK_VERSION,
            "created_at": CREATED_AT,
            "configuration": {
                "briefing_duration_seconds": 30,
                "negotiation_duration_seconds": 90,
                "commitment_duration_seconds": 30,
                "commitment_reminder_seconds_before_deadline": 10,
                "result_duration_seconds": 20,
                "roles": list(ROLES),
            },
            "fixture": copy.deepcopy(FIXTURE),
            "initial_state": copy.deepcopy(self.state),
            "initial_timer": {
                "timer_id": "phase_deadline",
                "generation": 1,
                "scheduled_for": "2026-08-15T12:00:30Z",
            },
        }
        self.genesis_hash = digest(self.genesis_body)
        self.head = self._head(0, self.genesis_hash)

    def _head(self, seq: int, lineage_hash: str) -> dict[str, Any]:
        core_hash = digest(self.core)
        activity_hash = digest(self.state)
        return {
            "room_id": ROOM_ID,
            "room_seq": seq,
            "lineage_hash": lineage_hash,
            "core_state_hash": core_hash,
            "activity_state_hash": activity_hash,
            "authoritative_state_hash": digest(
                {"core": core_hash, "activity": activity_hash}
            ),
        }

    def _attention(self, seq: int, reason: str) -> list[dict[str, Any]]:
        return [attention_signal(self.state, "broker", reason)]

    def transition(
        self,
        stimulus: dict[str, Any],
        event: dict[str, Any],
        attention: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        next_seq = len(self.transitions) + 1
        stimulus = copy.deepcopy(stimulus)
        stimulus["next_room_seq"] = next_seq
        before = copy.deepcopy(self.state)
        expected = reduce_state(before, stimulus)
        self.state = expected
        body = {
            "seq": next_seq,
            "stimulus": stimulus,
            "events": [event],
            "attention": attention or [],
            "state_after": copy.deepcopy(self.state),
            "state_hash": digest(self.state),
        }
        transition_hash = digest({"previous": self.head["lineage_hash"], **body})
        record = {**body, "transition_hash": transition_hash}
        self.transitions.append(record)
        self.head = self._head(next_seq, transition_hash)
        for signal in attention or []:
            if signal["target_member_id"] == MEMBERS["broker"]:
                self.activation.create_from_attention(
                    next_seq, signal, self.state, self.head
                )
        return record

    def evidence_record(self, record: dict[str, Any]) -> None:
        self.evidence.append({**record, "canonical": False})

    def action(
        self,
        role: str,
        action_type: str,
        action_id: str,
        admitted_at: str,
        payload: dict[str, Any],
        authority: str = "participant-action-capability",
    ) -> dict[str, Any]:
        if role not in ROLES:
            raise ValueError("action role is outside the story")
        if action_type not in ACTION_ORDER:
            raise ValueError("action type is outside the deterministic story")
        if not isinstance(action_id, str) or not action_id:
            raise ValueError("action identity is invalid")
        if not isinstance(admitted_at, str) or not admitted_at:
            raise ValueError("action admitted_at is invalid")
        if not isinstance(payload, dict):
            raise TypeError("action payload is invalid")
        stimulus = {
            "kind": "action",
            "member_id": MEMBERS[role],
            "role": role,
            "action_id": action_id,
            "action_type": action_type,
            "payload": payload,
            "admitted_at": admitted_at,
            "authority": authority,
        }
        prior = self.action_receipts.get(action_id)
        if prior is not None:
            prior_hash, prior_record = prior
            original_basis = prior_record["stimulus"]["next_room_seq"] - 1
            request_hash = digest(
                {
                    "room_id": ROOM_ID,
                    "member_id": MEMBERS[role],
                    "action_id": action_id,
                    "action_type": action_type,
                    "based_on_room_seq": original_basis,
                    "payload": payload,
                }
            )
            if prior_hash != request_hash:
                raise ValueError("idempotency conflict for action identity")
            self.last_action_was_duplicate = True
            return copy.deepcopy(prior_record)
        self.last_action_was_duplicate = False
        request_hash = digest(
            {
                "room_id": ROOM_ID,
                "member_id": MEMBERS[role],
                "action_id": action_id,
                "action_type": action_type,
                "based_on_room_seq": len(self.transitions),
                "payload": payload,
            }
        )
        event = {
            "event_type": f"{action_type}_accepted",
            "role": role,
            "action_id": action_id,
        }
        if action_type == "propose_plan":
            event["plan_id"] = action_id
        if action_type == "commit_move":
            event["sealed"] = True
        attention = None
        if action_type == "propose_plan":
            predicted = reduce_state(
                {**self.state}, {**stimulus, "next_room_seq": len(self.transitions) + 1}
            )
            attention = [attention_signal(predicted, "broker", "endorsement_requested")]
        record = self.transition(stimulus, event, attention)
        self.action_receipts[action_id] = (request_hash, copy.deepcopy(record))
        return record

    def timer(
        self,
        kind: str,
        timer_id: str,
        generation: int,
        scheduled_for: str,
        payload: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        if not isinstance(kind, str) or not kind:
            raise ValueError("timer kind is invalid")
        if not isinstance(timer_id, str) or not timer_id:
            raise ValueError("timer identity is invalid")
        if not isinstance(generation, int) or generation < 1:
            raise ValueError("timer generation is invalid")
        if not isinstance(scheduled_for, str) or not scheduled_for:
            raise ValueError("timer scheduled_for is invalid")
        if payload is not None and not isinstance(payload, dict):
            raise ValueError("timer payload is invalid")
        stimulus = {
            "kind": "timer",
            "timer": {
                "timer_id": timer_id,
                "generation": generation,
                "scheduled_for": scheduled_for,
                "kind": kind,
                "payload": payload or {},
            },
        }
        event_name = (
            "commitment_reminder" if kind == "commitment_reminder" else f"{kind}_fired"
        )
        attention: list[dict[str, Any]] = []
        if kind == "commitment_reminder":
            attention = self._attention(
                len(self.transitions) + 1, "required_action_deadline"
            )
        elif kind == "negotiation_deadline":
            predicted = reduce_state(
                {**self.state}, {**stimulus, "next_room_seq": len(self.transitions) + 1}
            )
            attention = [attention_signal(predicted, "broker", "commitment_opened")]
        elif kind == "resolve_now":
            predicted = reduce_state(
                {**self.state}, {**stimulus, "next_room_seq": len(self.transitions) + 1}
            )
            attention = [
                attention_signal(predicted, "broker", "round_result_available")
            ]
        return self.transition(stimulus, {"event_type": event_name}, attention)

    def finish(self) -> dict[str, Any]:
        final_outcome = copy.deepcopy(self.state["outcome"])
        replay = replay_story(self)
        transcript = {
            "schema": "worldstream/imo-57-absent-broker-story/v1",
            "description": "Offline protocol/story evidence; no live service execution.",
            "hash_algorithm": HASH_ALGORITHM,
            "canonical_json": "sorted UTF-8 JSON, no insignificant whitespace",
            "reference": {"pack_id": PACK_ID, "pack_version": PACK_VERSION},
            "fixture": copy.deepcopy(FIXTURE),
            "roles": {
                "navigator": "cooperative",
                "insider": "cautious",
                "broker": "withholding/absent runner; immutable seat remains present",
            },
            "phase_path": [self.genesis_body["initial_state"]["phase"]]
            + [item["state_after"]["phase"] for item in self.transitions],
            "genesis": {**self.genesis_body, "genesis_hash": self.genesis_hash},
            "transitions": self.transitions,
            "operational_evidence": [*self.activation.records, *self.evidence],
            "expected_fixture_outcome": final_outcome,
            "final_read_only_replay": replay,
            "final_head": self.head,
        }
        transcript["transcript_digest"] = digest(transcript)
        return transcript


def _replay_records(
    genesis_hash: str,
    initial_state: dict[str, Any],
    transitions: list[dict[str, Any]],
    core: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any], int, list[str]]:
    state = copy.deepcopy(initial_state)
    previous = genesis_hash
    activation_created = 0
    transition_hashes: list[str] = []
    for expected_seq, transition in enumerate(transitions, 1):
        if not isinstance(transition, dict) or set(transition) != {
            "seq",
            "stimulus",
            "events",
            "attention",
            "state_after",
            "state_hash",
            "transition_hash",
        }:
            raise ValueError(
                f"transition {expected_seq} does not match the evidence schema"
            )
        if transition["seq"] != expected_seq:
            raise ValueError("transitions are not strictly ordered")
        stimulus = transition["stimulus"]
        if (
            not isinstance(stimulus, dict)
            or stimulus.get("next_room_seq") != expected_seq
        ):
            raise ValueError("transition next sequence is invalid")
        kind = stimulus.get("kind")
        if kind == "action":
            if set(stimulus) != {
                "kind",
                "member_id",
                "role",
                "action_id",
                "action_type",
                "payload",
                "admitted_at",
                "authority",
                "next_room_seq",
            }:
                raise ValueError("action stimulus does not match the evidence schema")
            role = stimulus["role"]
            if role not in ROLES or stimulus["member_id"] != MEMBERS[role]:
                raise ValueError(
                    "action stimulus is outside the story membership scope"
                )
            if not isinstance(stimulus["action_id"], str) or not stimulus["action_id"]:
                raise ValueError("action identity is invalid")
            if stimulus["action_type"] not in ACTION_ORDER:
                raise ValueError("action type is outside the deterministic story")
            if not isinstance(stimulus["payload"], dict):
                raise ValueError("action payload is invalid")
            if (
                not isinstance(stimulus["admitted_at"], str)
                or not stimulus["admitted_at"]
            ):
                raise ValueError("action admitted_at is invalid")
            if not isinstance(stimulus["authority"], str) or not stimulus["authority"]:
                raise ValueError("action authority is invalid")
        elif kind == "timer":
            if set(stimulus) != {"kind", "timer", "next_room_seq"}:
                raise ValueError("timer stimulus does not match the evidence schema")
            timer = stimulus["timer"]
            if not isinstance(timer, dict) or set(timer) != {
                "timer_id",
                "generation",
                "scheduled_for",
                "kind",
                "payload",
            }:
                raise ValueError("timer stimulus is invalid")
            if not isinstance(timer["timer_id"], str) or not timer["timer_id"]:
                raise ValueError("timer identity is invalid")
            if not isinstance(timer["generation"], int) or timer["generation"] < 1:
                raise ValueError("timer generation is invalid")
            if (
                not isinstance(timer["scheduled_for"], str)
                or not timer["scheduled_for"]
            ):
                raise ValueError("timer scheduled_for is invalid")
            if not isinstance(timer["kind"], str) or not timer["kind"]:
                raise ValueError("timer kind is invalid")
            if not isinstance(timer["payload"], dict):
                raise ValueError("timer payload is invalid")
        else:
            raise ValueError("unknown deterministic stimulus kind")
        if not isinstance(transition["events"], list) or len(transition["events"]) != 1:
            raise ValueError("transition event set is invalid")
        event = transition["events"][0]
        if not isinstance(event, dict) or not isinstance(event.get("event_type"), str):
            raise TypeError("transition event is invalid")
        attention = transition["attention"]
        if not isinstance(attention, list) or len(attention) > len(ROLES):
            raise ValueError("transition attention set is invalid")
        for signal in attention:
            if not isinstance(signal, dict) or set(signal) != {
                "target_member_id",
                "reason",
                "priority",
                "deduplication_key",
                "deadline",
                "action_types",
            }:
                raise ValueError("attention signal is invalid")
            if signal["target_member_id"] != MEMBERS["broker"]:
                raise ValueError("offline story attention crossed its membership scope")
            if signal["reason"] not in ATTENTION_PRECEDENCE:
                raise ValueError("attention reason is invalid")
            if signal["priority"] != 1 or not isinstance(
                signal["deduplication_key"], str
            ):
                raise ValueError("attention priority or deduplication key is invalid")
            if not isinstance(signal["deadline"], str) or not isinstance(
                signal["action_types"], list
            ):
                raise TypeError("attention payload is invalid")
        state_after = reduce_state(state, stimulus)
        if state_after != transition["state_after"]:
            raise ValueError(f"replay reducer mismatch at sequence {expected_seq}")
        if transition["state_hash"] != digest(state_after):
            raise ValueError(f"replay state hash mismatch at sequence {expected_seq}")
        body = {
            key: transition[key]
            for key in (
                "seq",
                "stimulus",
                "events",
                "attention",
                "state_after",
                "state_hash",
            )
        }
        actual_hash = digest({"previous": previous, **body})
        if actual_hash != transition["transition_hash"]:
            raise ValueError(f"replay hash mismatch at sequence {expected_seq}")
        previous = actual_hash
        transition_hashes.append(actual_hash)
        state = state_after
        activation_created += len(attention)
    core_hash = digest(core)
    activity_hash = digest(state)
    head = {
        "room_id": ROOM_ID,
        "room_seq": len(transitions),
        "lineage_hash": previous,
        "core_state_hash": core_hash,
        "activity_state_hash": activity_hash,
        "authoritative_state_hash": digest(
            {"core": core_hash, "activity": activity_hash}
        ),
    }
    return state, head, activation_created, transition_hashes


def _replay_hash_comparison(
    expected_head: dict[str, Any],
    actual_head: dict[str, Any],
    expected_transition_hashes: list[str],
    actual_transition_hashes: list[str],
) -> dict[str, dict[str, Any]]:
    """Compare every canonical hash, including the complete Transition set."""

    comparison: dict[str, dict[str, Any]] = {}
    for field in (
        "core_state_hash",
        "activity_state_hash",
        "authoritative_state_hash",
        "lineage_hash",
    ):
        expected = expected_head.get(field)
        actual = actual_head.get(field)
        comparison[field] = {
            "expected": expected,
            "actual": actual,
            "match": expected == actual,
        }
    comparison["transition_hashes"] = {
        "expected": expected_transition_hashes,
        "actual": actual_transition_hashes,
        "match": expected_transition_hashes == actual_transition_hashes,
    }
    return comparison


def replay_story(story: Story) -> dict[str, Any]:
    state, head, activation_created, transition_hashes = _replay_records(
        story.genesis_hash, _initial_state(), story.transitions, story.core
    )
    if head != story.head:
        raise AssertionError("Replay head mismatch")
    comparison = _replay_hash_comparison(
        story.head,
        head,
        [transition["transition_hash"] for transition in story.transitions],
        transition_hashes,
    )
    if not all(item["match"] for item in comparison.values()):
        raise AssertionError("Replay hash comparison failed")
    return {
        "read_only": True,
        "source": "genesis_plus_ordered_transitions",
        "snapshots_used": 0,
        "activation_created": activation_created,
        "current_head": head,
        "replayed_activity_state_hash": digest(state),
        "verified_transition_count": len(story.transitions),
        "verified": True,
    }


@dataclass
class RetainedRoom:
    """A loaded retained Room with no mutable executor or Activation side effects."""

    transcript: dict[str, Any]
    state: dict[str, Any]
    head: dict[str, Any]
    next_transition_index: int
    executor_digest: str


@dataclass(frozen=True)
class RetainedHeistExecutor:
    """The exact executable retained for old Heist Room lineage."""

    pack_id: str = PACK_ID
    pack_version: str = PACK_VERSION
    pack_digest: str = PACK_DIGEST
    selectable_for_new_rooms: bool = True
    runnable_for_retained_rooms: bool = True

    def _check_identity(self, transcript: dict[str, Any]) -> None:
        if self.pack_id != PACK_ID or self.pack_version != PACK_VERSION:
            raise ValueError("retained Heist executor identity is unsupported")
        if self.pack_digest != PACK_DIGEST:
            raise ValueError("exact retained Heist executor digest is unavailable")
        reference = transcript.get("reference")
        if reference != {"pack_id": PACK_ID, "pack_version": PACK_VERSION}:
            raise ValueError("retained Room pack identity is inconsistent")
        if not self.runnable_for_retained_rooms:
            raise ValueError("retained Heist executor is not runnable")

    def load(
        self, transcript: dict[str, Any], at_room_seq: int | None = None
    ) -> RetainedRoom:
        """Load retained Genesis/Transitions through the exact executor."""

        validate_transcript(transcript)
        self._check_identity(transcript)
        transitions = transcript["transitions"]
        target = len(transitions) if at_room_seq is None else at_room_seq
        if not isinstance(target, int) or not 0 <= target <= len(transitions):
            raise ValueError("retained Room load sequence is outside the lineage")
        state, head, _, _ = _replay_records(
            transcript["genesis"]["genesis_hash"],
            transcript["genesis"]["initial_state"],
            transitions[:target],
            _core_state(),
        )
        return RetainedRoom(
            transcript=copy.deepcopy(transcript),
            state=state,
            head=head,
            next_transition_index=target,
            executor_digest=self.pack_digest,
        )

    def project(
        self, room: RetainedRoom, audience: str = "public", role: str | None = None
    ) -> dict[str, Any]:
        self._check_identity(room.transcript)
        if room.executor_digest != self.pack_digest:
            raise ValueError("loaded Room executor digest changed")
        return audience_projection(room.state, audience, role)

    def advance(self, room: RetainedRoom) -> dict[str, Any]:
        """Apply exactly one retained Transition and compare its stored bytes."""

        self._check_identity(room.transcript)
        if room.executor_digest != self.pack_digest:
            raise ValueError("loaded Room executor digest changed")
        transitions = room.transcript["transitions"]
        if room.next_transition_index >= len(transitions):
            raise ValueError("retained Room is already at its complete Head")
        target = room.next_transition_index + 1
        state, head, _, hashes = _replay_records(
            room.transcript["genesis"]["genesis_hash"],
            room.transcript["genesis"]["initial_state"],
            transitions[:target],
            _core_state(),
        )
        transition = transitions[target - 1]
        if hashes[-1] != transition["transition_hash"]:
            raise ValueError("retained Transition hash changed during advance")
        room.state = state
        room.head = head
        room.next_transition_index = target
        return copy.deepcopy(transition)

    def replay(self, room: RetainedRoom) -> dict[str, Any]:
        """Replay Genesis through all retained Transitions without Activation work."""

        self._check_identity(room.transcript)
        state, head, attention_replayed, hashes = _replay_records(
            room.transcript["genesis"]["genesis_hash"],
            room.transcript["genesis"]["initial_state"],
            room.transcript["transitions"],
            _core_state(),
        )
        expected_head = room.transcript["final_head"]
        comparison = _replay_hash_comparison(
            expected_head,
            head,
            [
                transition["transition_hash"]
                for transition in room.transcript["transitions"]
            ],
            hashes,
        )
        if not all(item["match"] for item in comparison.values()):
            raise ValueError("retained Replay hash comparison failed")
        if attention_replayed != 4:
            raise ValueError("retained Replay attempted unexpected Activation work")
        return {
            "read_only": True,
            "verified": True,
            "snapshots_used": 0,
            "source": "retained_genesis_plus_ordered_transitions",
            "verified_transition_count": len(hashes),
            "activation_created": 0,
            "attention_replayed": attention_replayed,
            "current_head": head,
            "hash_comparison": comparison,
            "replayed_activity_state_hash": digest(state),
        }


def retained_executor_self_test(transcript: dict[str, Any]) -> dict[str, Any]:
    """Exercise retained load, project, advance, and exact Replay locally."""

    executor = RetainedHeistExecutor()
    room = executor.load(transcript, at_room_seq=0)
    initial_projection = executor.project(room)
    advanced = 0
    while room.next_transition_index < len(transcript["transitions"]):
        executor.advance(room)
        advanced += 1
    final_projection = executor.project(room)
    replay = executor.replay(room)
    return {
        "pack_id": executor.pack_id,
        "pack_version": executor.pack_version,
        "pack_digest": executor.pack_digest,
        "selectable_for_new_rooms": executor.selectable_for_new_rooms,
        "runnable_for_retained_rooms": executor.runnable_for_retained_rooms,
        "operations": {
            "load": True,
            "project": True,
            "advance": True,
            "replay": True,
        },
        "loaded_from": "genesis",
        "initial_phase": initial_projection["phase"],
        "final_phase": final_projection["phase"],
        "phase_path": transcript["phase_path"],
        "advanced_transition_count": advanced,
        "replay": replay,
    }


def build_story() -> dict[str, Any]:
    story = Story()

    # Genesis has no transition, Attention, Frame, or Activation.
    story.evidence_record(
        {
            "kind": "genesis_marker",
            "attention_count": 0,
            "frame_count": 0,
            "activation_count": 0,
        }
    )
    story.evidence_record(
        {
            "kind": "recovery_marker",
            "marker": "kill_before_commit",
            "operation_id": "action:insider:commit-before-kill",
            "head_before": copy.deepcopy(story.head),
            "commit_observed": False,
            "recovery": "retry_identical_request",
        }
    )

    story.timer("briefing_deadline", "phase_deadline", 1, "2026-08-15T12:00:30Z")
    story.action(
        "navigator",
        "inspect_clue",
        "action-nav-inspect",
        "2026-08-15T12:00:31Z",
        {"clue_id": "route"},
    )
    story.action(
        "insider",
        "inspect_clue",
        "action-insider-inspect",
        "2026-08-15T12:00:32Z",
        {"clue_id": "entry_window"},
    )
    story.action(
        "navigator",
        "publish_clue",
        "action-nav-publish",
        "2026-08-15T12:00:33Z",
        {"clue_id": "route", "claim_code": "route_service"},
    )
    story.action(
        "navigator",
        "propose_plan",
        PLAN_ID,
        "2026-08-15T12:00:34Z",
        {
            "route": "service",
            "entry_window": "early",
            "required_tool": "thermal_key",
            "extraction": "boat",
        },
    )

    # Re-delivering the same Attention/offer is operationally idempotent. It
    # must not append a canonical transition or move the Room Cursor.
    first_attention = story.transitions[-1]["attention"][0]
    cursor_before_duplicate_offer = story.head["room_seq"]
    duplicate_offer_activation = story.activation.create_from_attention(
        story.head["room_seq"],
        first_attention,
        story.state,
        story.head,
    )
    story.evidence_record(
        {
            "kind": "duplicate_offer",
            "activation_id": duplicate_offer_activation,
            "outcome": "duplicate_existing",
            "cursor_before": cursor_before_duplicate_offer,
            "cursor_after": story.head["room_seq"],
            "no_new_transition": True,
        }
    )

    first_activation = next(
        activation_id
        for activation_id, item in story.activation.activations.items()
        if item["cause_seq"] == 5
    )
    first_claim = story.activation.claim(
        first_activation,
        "runner-broker-1",
        "claim:first-broker",
        digest({"activation_id": first_activation, "lease_generation": 0}),
        "2026-08-15T12:00:35Z",
        "2026-08-15T12:00:45Z",
        0,
    )
    story.evidence_record(
        {
            "kind": "broker_invocation",
            "activation_id": first_activation,
            "claim_outcome": first_claim["outcome"],
            "invocation": "broker-invocation-1",
            "outcome": "ended_before_commitment",
            "participant_action_submitted": False,
        }
    )
    story.activation.complete(
        first_activation,
        "complete:first-broker",
        digest(
            {
                "activation_id": first_activation,
                "result_code": "runner_exited_before_commitment",
            }
        ),
        first_claim["lease_generation"],
        "runner_exited_before_commitment",
    )
    story.action(
        "insider",
        "endorse_plan",
        "action-insider-endorse",
        "2026-08-15T12:00:36Z",
        {"plan_id": PLAN_ID},
    )
    story.timer("negotiation_deadline", "phase_deadline", 2, "2026-08-15T12:02:00Z")

    # Commitment opening is the fresh Attention. Claiming it never grants a
    # participant credential, Cursor advancement, or Action authority.
    fresh_activation = next(
        activation_id
        for activation_id, item in story.activation.activations.items()
        if item["cause_seq"] == 7
    )
    fresh = story.activation.claim(
        fresh_activation,
        "runner-broker-2",
        "claim:fresh-broker",
        digest({"activation_id": fresh_activation, "lease_generation": 0}),
        "2026-08-15T12:02:01Z",
        "2026-08-15T12:02:05Z",
        0,
    )
    story.evidence_record(
        {
            "kind": "lost_claim_reply",
            "activation_id": fresh_activation,
            "first_claim_committed": True,
            "reply_delivered": False,
            "retry": "same_operation_identity_and_request_hash",
        }
    )
    duplicate_claim = story.activation.claim(
        fresh_activation,
        "runner-broker-2",
        "claim:fresh-broker",
        digest({"activation_id": fresh_activation, "lease_generation": 0}),
        "2026-08-15T12:02:02Z",
        "2026-08-15T12:02:05Z",
        0,
    )
    story.evidence_record(
        {
            "kind": "fresh_activation_context",
            "activation_id": fresh_activation,
            "context_hash": fresh["context_hash"],
            "context": fresh["context"],
            "participant_action_authority": "not_granted_by_claim",
        }
    )
    reclaim = story.activation.expire_and_reclaim(
        fresh_activation, "2026-08-15T12:02:06Z", "runner-broker-3"
    )
    stale_completion = story.activation.complete(
        fresh_activation,
        "complete:old-broker-claim",
        digest({"activation_id": fresh_activation, "result_code": "no_action"}),
        fresh["lease_generation"],
        "no_action",
    )
    reclaimed_generation = story.activation.activations[fresh_activation][
        "lease_generation"
    ]
    story.activation.complete(
        fresh_activation,
        "complete:reclaimed-broker",
        digest({"activation_id": fresh_activation, "result_code": "no_action"}),
        reclaimed_generation,
        "no_action",
    )
    idempotent_completion = story.activation.complete(
        fresh_activation,
        "complete:reclaimed-broker",
        digest({"activation_id": fresh_activation, "result_code": "no_action"}),
        reclaimed_generation,
        "no_action",
    )
    story.evidence_record(
        {
            "kind": "activation_outcomes",
            "duplicate_claim": duplicate_claim["outcome"],
            "lost_claim_reply": "retry_resolved_original_receipt"
            if duplicate_claim["outcome"] == "duplicate_existing"
            else "unexpected",
            "lease_reclaim": reclaim["outcome"],
            "stale_generation": stale_completion["outcome"],
            "idempotent_completion": idempotent_completion["outcome"],
        }
    )

    story.action(
        "navigator",
        "commit_move",
        "action-nav-commit",
        "2026-08-15T12:02:03Z",
        {"selected_plan_id": PLAN_ID, "contribute_required_resource": False},
    )
    story.action(
        "insider",
        "commit_move",
        "action-insider-commit",
        "2026-08-15T12:02:04Z",
        {"selected_plan_id": PLAN_ID, "contribute_required_resource": True},
    )
    story.action(
        "insider",
        "commit_move",
        "action-insider-commit",
        "2026-08-15T12:02:04Z",
        {"selected_plan_id": PLAN_ID, "contribute_required_resource": True},
    )
    if not story.last_action_was_duplicate:
        raise AssertionError("duplicate Action did not resolve its original receipt")
    story.evidence_record(
        {
            "kind": "participant_authority",
            "action_id": "action-insider-commit",
            "member_id": MEMBERS["insider"],
            "authority": "participant-action-capability",
            "activation_id": fresh_activation,
            "relationship": "separate_authority; activation claim did not submit this Action",
        }
    )
    story.evidence_record(
        {
            "kind": "duplicate_action",
            "action_id": "action-insider-commit",
            "outcome": "existing_original_receipt_no_new_transition",
        }
    )
    story.timer(
        "commitment_reminder",
        "commitment_reminder",
        1,
        "2026-08-15T12:02:20Z",
        {"phase_generation": 3},
    )
    story.evidence_record(
        {
            "kind": "stale_generation",
            "timer_id": "phase_deadline",
            "provided_generation": 2,
            "current_generation": 3,
            "outcome": "not_applicable_no_transition",
        }
    )
    story.timer("commitment_deadline", "phase_deadline", 3, "2026-08-15T12:02:30Z")
    story.timer("resolve_now", "resolve_now", 1, "2026-08-15T12:02:30.000001Z")
    story.action(
        "navigator", "acknowledge_result", "action-nav-ack", "2026-08-15T12:02:31Z", {}
    )
    story.action(
        "insider",
        "acknowledge_result",
        "action-insider-ack",
        "2026-08-15T12:02:32Z",
        {},
    )
    story.timer("result_deadline", "phase_deadline", 4, "2026-08-15T12:02:50.000001Z")
    story.evidence_record(
        {
            "kind": "recovery_marker",
            "marker": "kill_after_commit_before_publication",
            "operation_id": "action:insider:commit",
            "commit_observed": True,
            "publication_observed": False,
            "recovery": "resolve_original_receipt_and_publish_existing_transition",
        }
    )
    return story.finish()


EXPECTED_OUTCOME = {
    "outcome": "success",
    "selected_plan_id": PLAN_ID,
    "vote_counts": {PLAN_ID: 2},
    "missing_roles": ["broker"],
    "checks": {
        "route": True,
        "entry_window": True,
        "required_tool": True,
        "extraction": True,
        "resource_contributed": True,
    },
    "score": 5,
    "reason": "scored_selected_plan",
}


def self_test() -> dict[str, Any]:
    transcript = build_story()
    validate_transcript(transcript)
    retained = retained_executor_self_test(transcript)
    assert transcript["expected_fixture_outcome"] == EXPECTED_OUTCOME
    assert transcript["phase_path"][-1] == "Complete"
    assert transcript["final_read_only_replay"]["verified"] is True
    assert transcript["final_read_only_replay"]["activation_created"] == 4
    assert len(transcript["transitions"]) == 15
    assert transcript["transcript_digest"].startswith("sha256:")
    contexts = [
        item
        for item in transcript["operational_evidence"]
        if item.get("kind") == "fresh_activation_context"
    ]
    assert len(contexts) == 1
    assert contexts[0]["context_hash"] == digest(contexts[0]["context"])
    assert retained["pack_digest"] == PACK_DIGEST
    assert retained["operations"] == {
        "load": True,
        "project": True,
        "advance": True,
        "replay": True,
    }
    assert retained["phase_path"] == transcript["phase_path"]
    assert retained["replay"]["verified"] is True
    assert all(item["match"] for item in retained["replay"]["hash_comparison"].values())
    outcomes = next(
        item
        for item in transcript["operational_evidence"]
        if item.get("kind") == "activation_outcomes"
    )
    assert outcomes == {
        "kind": "activation_outcomes",
        "canonical": False,
        "duplicate_claim": "duplicate_existing",
        "lost_claim_reply": "retry_resolved_original_receipt",
        "lease_reclaim": "reclaimed",
        "stale_generation": "stale_generation",
        "idempotent_completion": "idempotent_existing",
    }
    parity_path = Path(__file__).with_name(PARITY_FIXTURE)
    parity = json.loads(parity_path.read_text(encoding="utf-8"))
    assert parity["transcript_digest"] == transcript["transcript_digest"]
    assert parity["phase_path"] == transcript["phase_path"]
    assert parity["final_head"] == transcript["final_head"]
    assert parity["expected_fixture_outcome"] == transcript["expected_fixture_outcome"]
    assert parity["replay"]["verified_transition_count"] == len(
        transcript["transitions"]
    )
    retained_fixture = parity["retained_executor"]
    assert retained_fixture["pack_id"] == retained["pack_id"]
    assert retained_fixture["pack_version"] == retained["pack_version"]
    assert retained_fixture["pack_digest"] == retained["pack_digest"]
    assert (
        retained_fixture["selectable_for_new_rooms"]
        == retained["selectable_for_new_rooms"]
    )
    assert (
        retained_fixture["runnable_for_retained_rooms"]
        == retained["runnable_for_retained_rooms"]
    )
    assert retained_fixture["operations"] == retained["operations"]
    assert retained_fixture["phase_path"] == retained["phase_path"]
    assert retained_fixture["replay_hashes"] == {
        field: retained["replay"]["hash_comparison"][field]["expected"]
        for field in (
            "core_state_hash",
            "activity_state_hash",
            "authoritative_state_hash",
            "lineage_hash",
            "transition_hashes",
        )
    }
    assert [
        phase
        for phase in (
            "Briefing",
            "Negotiation",
            "Commitment",
            "Resolution",
            "Result",
            "Complete",
        )
        if phase in transcript["phase_path"]
    ] == [
        "Briefing",
        "Negotiation",
        "Commitment",
        "Resolution",
        "Result",
        "Complete",
    ]
    return transcript
