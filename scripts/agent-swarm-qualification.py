#!/usr/bin/env python3
"""Fail-closed assembler for native mixed-provider Agent Swarm evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
from itertools import pairwise
from pathlib import Path, PurePosixPath

EVIDENCE_SCHEMA = "worldstream/agent-swarm-qualification-evidence/v2"
REPORT_SCHEMA = "worldstream/agent-swarm-qualification-report/v2"
PROVIDER_QUALIFICATION_SCHEMA = "worldstream/agent-swarm-provider-qualification/v2"
INSTALL_SCHEMA = "worldstream/agent-swarm-install/v1"
PROVIDER_INVOCATION_SCHEMA = "worldstream/agent-swarm-provider-invocation-receipt/v1"
PROVIDER_LOGIN_SCHEMA = "worldstream/agent-swarm-provider-login-receipt/v1"
SCHEDULER_TRACE_SCHEMA = "worldstream/agent-swarm-scheduler-trace/v1"
PROCESS_CONTAINMENT_SCHEMA = "worldstream/agent-swarm-process-containment/v1"
RESOURCE_CONFINEMENT_SCHEMA = "worldstream/agent-swarm-resource-confinement/v1"
SCENARIO_RECEIPT_SCHEMA = "worldstream/agent-swarm-scenario-receipt/v1"
PACK_ID = "worldstream.agent-swarm"
COORDINATION_ENGINE = "worldstream-agent-swarm"
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
GIT_REVISION = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})\Z")
BLAKE3 = re.compile(r"blake3:[0-9a-f]{64}\Z")
ADVERTISED_TARGETS = {
    "macos-arm64": ("macos", "arm64"),
    "macos-x86_64": ("macos", "x86_64"),
    "windows-x64": ("windows", "x86_64"),
}
PROVIDERS = {"codex", "claude", "kiro"}
VERDICTS = {"pass", "fail", "unsupported"}
SCENARIOS = {
    "sourced_report",
    "reviewed_code_change",
    "explicit_provider_settings",
    "unavailable_settings_blocked",
    "concurrent_swarms_caps_priorities",
    "progress_review_priority",
    "budget_pause",
    "steering_reassignment",
    "three_failed_corrections",
    "review_dispute",
    "shared_resource_conflict",
    "unknown_effect",
    "owned_descendant_cleanup",
    "detach_reattach",
    "crash_recovery_explicit_resume",
    "completion_quiescence",
    "late_result_fencing",
    "same_room_reopen",
    "terminal_interaction",
    "pack_upgrade_preserves_history",
}
RESULT_SCENARIOS = {
    "sourced_report": "report",
    "reviewed_code_change": "code_change",
}
PROVIDER_ASSERTIONS = {
    "subscription_login",
    "deterministic_contract",
    "effective_configuration_report",
    "delegation_contained",
    "native_cancellation",
    "owned_descendant_containment",
    "resource_confinement",
    "same_provider_setting_isolation",
    "next_invocation_setting_change",
    "unavailable_or_substituted_setting_blocks",
}
MIN_ROSTER_SIZE = 9
MAX_ROSTER_SIZE = 16
MAX_PRIORITY = 16
MAX_EVIDENCE_REFS = 64
MAX_RECEIPT_BYTES = 1024 * 1024
RECEIPT_KINDS = {
    INSTALL_SCHEMA: "install",
    PROVIDER_INVOCATION_SCHEMA: "provider_invocation",
    PROVIDER_LOGIN_SCHEMA: "provider_login",
    SCHEDULER_TRACE_SCHEMA: "scheduler_trace",
    PROCESS_CONTAINMENT_SCHEMA: "process_containment",
    RESOURCE_CONFINEMENT_SCHEMA: "resource_confinement",
    SCENARIO_RECEIPT_SCHEMA: "scenario",
}
DIRECT_API_ENVIRONMENT = {
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "ANTHROPIC_VERTEX_BASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_OPENAI_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_VERTEX",
    "CODEX_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "KIRO_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
}


class QualificationError(RuntimeError):
    """Evidence is incomplete, contradictory, duplicated, or malformed."""


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def receipt_manifest_sha256(manifest: dict[str, object]) -> str:
    """Digest a manifest with a language-neutral, length-unambiguous encoding."""

    digest = hashlib.sha256()
    for receipt_id in sorted(manifest):
        entry = manifest[receipt_id]
        for value in (
            receipt_id,
            entry["path"],
            entry["sha256"],
            entry["schema"],
            entry["kind"],
        ):
            encoded = str(value).encode()
            digest.update(len(encoded).to_bytes(8, "big"))
            digest.update(encoded)
    return digest.hexdigest()


def _object(value: object, label: str, keys: set[str]) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != keys:
        raise QualificationError(f"{label} must contain exactly {sorted(keys)}")
    return value


def _nonempty(value: object, label: str) -> str:
    if not isinstance(value, str) or not value.strip() or len(value) > 4096:
        raise QualificationError(f"{label} must be a bounded non-empty string")
    return value


def _optional_nonempty(value: object, label: str) -> str | None:
    if value is None:
        return None
    return _nonempty(value, label)


def _integer(value: object, label: str, minimum: int, maximum: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < minimum
        or value > maximum
    ):
        raise QualificationError(
            f"{label} must be an integer from {minimum} through {maximum}"
        )
    return value


def _optional_positive_integer(value: object, label: str) -> int | None:
    if value is None:
        return None
    return _integer(value, label, 1, 2**63 - 1)


def _digest(value: object, label: str) -> str:
    value = _nonempty(value, label)
    if not SHA256.fullmatch(value):
        raise QualificationError(f"{label} must be lowercase SHA-256")
    return value


def _blake3(value: object, label: str) -> str:
    value = _nonempty(value, label)
    if not BLAKE3.fullmatch(value):
        raise QualificationError(f"{label} must be a lowercase BLAKE3 reference")
    return value


def _read_bounded_regular_json(
    path: Path, label: str
) -> tuple[bytes, dict[str, object]]:
    try:
        info = path.lstat()
    except OSError as error:
        raise QualificationError(f"cannot read {label}: {path}") from error
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise QualificationError(f"{label} must be a regular non-symlink file")
    if info.st_size <= 0 or info.st_size > MAX_RECEIPT_BYTES:
        raise QualificationError(f"{label} size is invalid")
    try:
        raw = path.read_bytes()
        value = json.loads(raw)
    except (OSError, json.JSONDecodeError) as error:
        raise QualificationError(f"{label} is not readable JSON") from error
    if not isinstance(value, dict):
        raise QualificationError(f"{label} must be a JSON object")
    return raw, value


class ReceiptStore:
    """Bounded content-addressed receipt manifest for one native evidence file."""

    def __init__(self, manifest: object, evidence_path: Path):
        if not isinstance(manifest, dict) or not manifest or len(manifest) > 512:
            raise QualificationError(
                "receipt_manifest must be a bounded non-empty object"
            )
        try:
            base = evidence_path.parent.resolve(strict=True)
        except OSError as error:
            raise QualificationError(
                "evidence parent directory is unavailable"
            ) from error
        self.entries: dict[str, dict[str, object]] = {}
        self.documents: dict[str, dict[str, object]] = {}
        self.used: set[str] = set()
        seen_paths: set[Path] = set()
        for receipt_id, raw_entry in manifest.items():
            _nonempty(receipt_id, "receipt_manifest id")
            entry = _object(
                raw_entry,
                f"receipt_manifest.{receipt_id}",
                {"path", "sha256", "schema", "kind"},
            )
            relative = _nonempty(entry["path"], f"receipt_manifest.{receipt_id}.path")
            pure = PurePosixPath(relative)
            if (
                pure.is_absolute()
                or not pure.parts
                or any(part in {"", ".", ".."} for part in pure.parts)
                or "\\" in relative
            ):
                raise QualificationError(
                    f"receipt_manifest.{receipt_id}.path is not a safe relative path"
                )
            candidate = base
            try:
                for part in pure.parts:
                    candidate /= part
                    if stat.S_ISLNK(candidate.lstat().st_mode):
                        raise QualificationError(
                            f"receipt_manifest.{receipt_id}.path contains a symlink"
                        )
            except QualificationError:
                raise
            except OSError as error:
                raise QualificationError(
                    f"receipt_manifest.{receipt_id} is unavailable"
                ) from error
            try:
                receipt_path = candidate.resolve(strict=True)
            except OSError as error:
                raise QualificationError(
                    f"receipt_manifest.{receipt_id} is unavailable"
                ) from error
            if not receipt_path.is_relative_to(base):
                raise QualificationError(
                    f"receipt_manifest.{receipt_id}.path escapes the evidence directory"
                )
            if receipt_path in seen_paths:
                raise QualificationError("receipt_manifest aliases one receipt path")
            seen_paths.add(receipt_path)
            raw, document = _read_bounded_regular_json(
                receipt_path, f"receipt {receipt_id}"
            )
            expected_sha256 = _digest(
                entry["sha256"], f"receipt_manifest.{receipt_id}.sha256"
            )
            if hashlib.sha256(raw).hexdigest() != expected_sha256:
                raise QualificationError(
                    f"receipt_manifest.{receipt_id} content digest mismatch"
                )
            schema = _nonempty(entry["schema"], f"receipt_manifest.{receipt_id}.schema")
            kind = _nonempty(entry["kind"], f"receipt_manifest.{receipt_id}.kind")
            if RECEIPT_KINDS.get(schema) != kind or document.get("schema") != schema:
                raise QualificationError(
                    f"receipt_manifest.{receipt_id} schema/kind mismatch"
                )
            self.entries[receipt_id] = entry
            self.documents[receipt_id] = document

    def references(
        self,
        value: object,
        label: str,
        *,
        allowed_kinds: set[str] | None = None,
    ) -> list[str]:
        if not isinstance(value, list) or not value or len(value) > MAX_EVIDENCE_REFS:
            raise QualificationError(f"{label} must be a bounded non-empty array")
        refs = [_nonempty(item, f"{label}[]") for item in value]
        if len(set(refs)) != len(refs):
            raise QualificationError(f"{label} contains duplicate references")
        for receipt_id in refs:
            entry = self.entries.get(receipt_id)
            if entry is None:
                raise QualificationError(f"{label} names an unmanifested receipt")
            if allowed_kinds is not None and entry["kind"] not in allowed_kinds:
                raise QualificationError(f"{label} names an incompatible receipt kind")
            self.used.add(receipt_id)
        return refs

    def document(self, receipt_id: object, kind: str, label: str) -> dict[str, object]:
        receipt_id = _nonempty(receipt_id, label)
        entry = self.entries.get(receipt_id)
        if entry is None or entry["kind"] != kind:
            raise QualificationError(
                f"{label} does not name a manifested {kind} receipt"
            )
        self.used.add(receipt_id)
        return self.documents[receipt_id]

    def ensure_all_used(self) -> None:
        unused = sorted(set(self.entries) - self.used)
        if unused:
            raise QualificationError(
                f"receipt_manifest contains unreferenced receipts: {', '.join(unused)}"
            )


def _evidence_refs(
    value: object,
    label: str,
    receipts: ReceiptStore,
    *,
    allowed_kinds: set[str] | None = None,
) -> list[str]:
    return receipts.references(value, label, allowed_kinds=allowed_kinds)


def _validate_package(
    value: object, platform: dict[str, object], receipts: ReceiptStore
) -> dict[str, object]:
    package = _object(
        value,
        "package",
        {
            "sha256",
            "target",
            "application_version",
            "install_schema",
            "install_sha256",
            "install_ref",
            "pack_id",
            "pack_version",
            "pack_digest",
        },
    )
    _digest(package["sha256"], "package.sha256")
    target = _nonempty(package["target"], "package.target")
    if target not in ADVERTISED_TARGETS:
        raise QualificationError("package.target is not an advertised native target")
    expected_os, expected_arch = ADVERTISED_TARGETS[target]
    if platform["os"] != expected_os or platform["arch"] != expected_arch:
        raise QualificationError("package target does not match the native platform")
    _nonempty(package["application_version"], "package.application_version")
    if package["install_schema"] != INSTALL_SCHEMA:
        raise QualificationError("package install schema mismatch")
    install_sha256 = _digest(package["install_sha256"], "package.install_sha256")
    if package["pack_id"] != PACK_ID:
        raise QualificationError("package Pack identifier mismatch")
    _nonempty(package["pack_version"], "package.pack_version")
    _blake3(package["pack_digest"], "package.pack_digest")
    install_ref = _nonempty(package["install_ref"], "package.install_ref")
    install = receipts.document(install_ref, "install", "package.install_ref")
    if receipts.entries[install_ref]["sha256"] != install_sha256:
        raise QualificationError("package install receipt digest mismatch")
    install = _object(
        install,
        "package install receipt",
        {
            "schema",
            "application_version",
            "target",
            "execution_default",
            "provider_management",
            "state_location",
            "pack",
            "files",
        },
    )
    install_pack = _object(
        install["pack"],
        "package install receipt Pack",
        {"id", "version", "digest", "path"},
    )
    if (
        install["schema"] != INSTALL_SCHEMA
        or install["application_version"] != package["application_version"]
        or install["target"] != target
        or install["execution_default"] != "suspended_until_explicit_resume"
        or install["provider_management"]
        != "discover_only_never_install_or_reconfigure"
        or install_pack["id"] != package["pack_id"]
        or install_pack["version"] != package["pack_version"]
        or install_pack["digest"] != package["pack_digest"]
    ):
        raise QualificationError("package fields do not match the install receipt")
    _nonempty(install["state_location"], "package install receipt state_location")
    _nonempty(install_pack["path"], "package install receipt pack.path")
    if not isinstance(install["files"], dict) or not install["files"]:
        raise QualificationError("package install receipt file inventory is invalid")
    return package


def _validate_receipt_binding(
    document: dict[str, object],
    label: str,
    schema: str,
    binding: dict[str, str],
    payload_keys: set[str],
) -> dict[str, object]:
    receipt = _object(
        document,
        label,
        {
            "schema",
            "run_id",
            "target",
            "package_sha256",
            "install_sha256",
            "engine_instance_id",
            *payload_keys,
        },
    )
    if receipt["schema"] != schema:
        raise QualificationError(f"{label} schema mismatch")
    for field in (
        "run_id",
        "target",
        "package_sha256",
        "install_sha256",
        "engine_instance_id",
    ):
        if receipt[field] != binding[field]:
            raise QualificationError(f"{label} {field} binding mismatch")
    return receipt


def _scheduler_facts(
    document: dict[str, object], binding: dict[str, str]
) -> dict[str, object]:
    receipt = _validate_receipt_binding(
        document,
        "scheduler trace receipt",
        SCHEDULER_TRACE_SCHEMA,
        binding,
        {
            "machine_invocation_limit",
            "progress_review_interval_seconds",
            "slow_worker_delay_ms",
            "events",
        },
    )
    machine_limit = _integer(
        receipt["machine_invocation_limit"],
        "scheduler trace machine_invocation_limit",
        1,
        MAX_ROSTER_SIZE,
    )
    interval = _integer(
        receipt["progress_review_interval_seconds"],
        "scheduler trace progress_review_interval_seconds",
        1,
        86_400,
    )
    slow_worker_delay_ms = _integer(
        receipt["slow_worker_delay_ms"],
        "scheduler trace slow_worker_delay_ms",
        1,
        86_400_000,
    )
    events = receipt["events"]
    if not isinstance(events, list) or not events or len(events) > 100_000:
        raise QualificationError(
            "scheduler trace events must be a bounded non-empty array"
        )
    event_keys = {
        "sequence",
        "time_ms",
        "round",
        "event",
        "swarm_id",
        "invocation_id",
        "member_key",
        "provider",
        "kind",
        "provider_cap",
        "priority",
        "priority_source",
        "budget_invocation_limit",
        "budget_active_time_limit_ms",
        "desired",
        "phase",
    }
    event_names = {
        "snapshot",
        "queued",
        "eligible",
        "admitted",
        "completed",
        "timer_due",
        "review_claimed",
        "budget_threshold",
        "desired_changed",
    }
    active: dict[str, tuple[str, int]] = {}
    intervals: dict[str, tuple[str, int, int]] = {}
    provider_maximum = {provider: 0 for provider in PROVIDERS}
    maximum = 0
    provider_cap_values: dict[str, set[int]] = {
        provider: set() for provider in PROVIDERS
    }
    eligible_by_round: dict[int, list[dict[str, object]]] = {}
    admitted_by_round: dict[int, list[dict[str, object]]] = {}
    due_review_ids: set[str] = set()
    claimed_review_ids: set[str] = set()
    admitted_review_ids: set[str] = set()
    threshold_events: list[tuple[int, dict[str, object]]] = []
    desired_events: list[tuple[int, dict[str, object]]] = []
    observed_budget: tuple[int | None, int | None] | None = None
    previous_sequence = -1
    previous_time = -1
    validated_events: list[dict[str, object]] = []
    for index, raw_event in enumerate(events):
        event = _object(raw_event, f"scheduler trace events[{index}]", event_keys)
        sequence = _integer(
            event["sequence"], f"scheduler trace events[{index}].sequence", 0, 2**63 - 1
        )
        time_ms = _integer(
            event["time_ms"], f"scheduler trace events[{index}].time_ms", 0, 2**63 - 1
        )
        if sequence <= previous_sequence or time_ms < previous_time:
            raise QualificationError("scheduler trace events are not strictly ordered")
        previous_sequence = sequence
        previous_time = time_ms
        round_value = event["round"]
        if round_value is not None:
            round_value = _integer(
                round_value, f"scheduler trace events[{index}].round", 0, 2**63 - 1
            )
        event_name = event["event"]
        if event_name not in event_names:
            raise QualificationError("scheduler trace event kind is invalid")
        for field in ("swarm_id", "invocation_id", "member_key"):
            _optional_nonempty(event[field], f"scheduler trace events[{index}].{field}")
        provider = event["provider"]
        if provider is not None and provider not in PROVIDERS:
            raise QualificationError("scheduler trace provider is invalid")
        invocation_kind = event["kind"]
        if invocation_kind is not None and invocation_kind not in {
            "work",
            "progress_review",
        }:
            raise QualificationError("scheduler trace invocation kind is invalid")
        provider_cap = event["provider_cap"]
        if provider_cap is not None:
            provider_cap = _integer(
                provider_cap,
                f"scheduler trace events[{index}].provider_cap",
                0,
                MAX_ROSTER_SIZE,
            )
            if provider is None:
                raise QualificationError("scheduler trace cap lacks a provider")
            provider_cap_values[str(provider)].add(provider_cap)
        priority = event["priority"]
        if priority is not None:
            _integer(
                priority,
                f"scheduler trace events[{index}].priority",
                1,
                MAX_PRIORITY,
            )
        if event["priority_source"] is not None and event["priority_source"] not in {
            "default",
            "explicit",
        }:
            raise QualificationError("scheduler trace priority source is invalid")
        invocation_limit = _optional_positive_integer(
            event["budget_invocation_limit"],
            f"scheduler trace events[{index}].budget_invocation_limit",
        )
        active_time_limit = _optional_positive_integer(
            event["budget_active_time_limit_ms"],
            f"scheduler trace events[{index}].budget_active_time_limit_ms",
        )
        if invocation_limit is not None or active_time_limit is not None:
            candidate_budget = (invocation_limit, active_time_limit)
            if observed_budget is None:
                observed_budget = candidate_budget
            elif observed_budget != candidate_budget:
                raise QualificationError("scheduler trace budget snapshot changed")
        if event["desired"] is not None and event["desired"] not in {
            "running",
            "paused",
            "stopped",
        }:
            raise QualificationError("scheduler trace desired state is invalid")
        if event["phase"] is not None and event["phase"] not in {
            "running",
            "pausing",
            "paused",
            "stopping",
            "stopped",
            "recovery_required",
            "blocked_unknown",
        }:
            raise QualificationError("scheduler trace phase is invalid")

        invocation_id = event["invocation_id"]
        if event_name == "admitted":
            if (
                not isinstance(invocation_id, str)
                or provider not in PROVIDERS
                or invocation_kind not in {"work", "progress_review"}
                or not isinstance(event["swarm_id"], str)
                or not isinstance(event["member_key"], str)
                or invocation_id in active
            ):
                raise QualificationError(
                    "scheduler admission is incomplete or duplicated"
                )
            active[invocation_id] = (str(provider), time_ms)
            maximum = max(maximum, len(active))
            for provider_name in PROVIDERS:
                concurrent = sum(
                    1 for item in active.values() if item[0] == provider_name
                )
                provider_maximum[provider_name] = max(
                    provider_maximum[provider_name], concurrent
                )
            if round_value is not None:
                admitted_by_round.setdefault(round_value, []).append(event)
            if invocation_kind == "progress_review":
                admitted_review_ids.add(invocation_id)
        elif event_name == "completed":
            if not isinstance(invocation_id, str) or invocation_id not in active:
                raise QualificationError(
                    "scheduler completion lacks an active invocation"
                )
            active_provider, started_at_ms = active.pop(invocation_id)
            intervals[invocation_id] = (active_provider, started_at_ms, time_ms)
        elif event_name == "eligible":
            if round_value is None or not isinstance(invocation_id, str):
                raise QualificationError("scheduler eligibility lacks round/invocation")
            eligible_by_round.setdefault(round_value, []).append(event)
        elif event_name == "timer_due":
            if invocation_kind != "progress_review" or not isinstance(
                invocation_id, str
            ):
                raise QualificationError("scheduler due-review event is incomplete")
            due_review_ids.add(invocation_id)
        elif event_name == "review_claimed":
            if invocation_kind != "progress_review" or not isinstance(
                invocation_id, str
            ):
                raise QualificationError("scheduler review claim is incomplete")
            claimed_review_ids.add(invocation_id)
        elif event_name == "budget_threshold":
            if not isinstance(event["swarm_id"], str) or (
                invocation_limit is None and active_time_limit is None
            ):
                raise QualificationError("scheduler budget threshold is incomplete")
            threshold_events.append((index, event))
        elif event_name == "desired_changed":
            if not isinstance(event["swarm_id"], str):
                raise QualificationError("scheduler desired change lacks a Swarm")
            desired_events.append((index, event))
        validated_events.append(event)
    if active:
        raise QualificationError("scheduler trace ends with unterminated invocations")

    equal_priority = False
    default_priority = None
    explicit_equal_priority = None
    progress_review_priority = False
    for round_value, eligible in eligible_by_round.items():
        admissions = admitted_by_round.get(round_value, [])
        admitted_ids = {item["invocation_id"] for item in admissions}
        defaults = [item for item in eligible if item["priority_source"] == "default"]
        explicits = [item for item in eligible if item["priority_source"] == "explicit"]
        if defaults and default_priority is None:
            default_priority = defaults[0]["priority"]
        if explicits and explicit_equal_priority is None:
            explicit_equal_priority = explicits[0]["priority"]
        for default in defaults:
            for explicit in explicits:
                if (
                    default["swarm_id"] != explicit["swarm_id"]
                    and default["priority"] == explicit["priority"]
                    and default["invocation_id"] in admitted_ids
                    and explicit["invocation_id"] in admitted_ids
                ):
                    equal_priority = True
                    default_priority = default["priority"]
                    explicit_equal_priority = explicit["priority"]
        kinds = {item["kind"] for item in eligible}
        if {"work", "progress_review"}.issubset(kinds) and admissions:
            progress_review_priority = (
                progress_review_priority or admissions[0]["kind"] == "progress_review"
            )

    budget_pause = False
    for threshold_index, threshold in threshold_events:
        swarm_id = threshold["swarm_id"]
        paused = any(
            index > threshold_index
            and item["swarm_id"] == swarm_id
            and item["desired"] == "paused"
            and item["phase"] in {"pausing", "paused"}
            for index, item in desired_events
        )
        admitted_after = any(
            index > threshold_index
            and item["event"] == "admitted"
            and item["swarm_id"] == swarm_id
            for index, item in enumerate(validated_events)
        )
        if paused and not admitted_after:
            budget_pause = True

    due_completed = due_review_ids & claimed_review_ids & admitted_review_ids
    provider_caps = {
        provider: (max(values) if values else 0)
        for provider, values in provider_cap_values.items()
    }
    return {
        "machine_invocation_limit": machine_limit,
        "maximum_simultaneous_invocations": maximum,
        "provider_caps": provider_caps,
        "provider_maximum": provider_maximum,
        "intervals": intervals,
        "default_priority": default_priority,
        "explicit_equal_priority": explicit_equal_priority,
        "equal_priority_order_observed": equal_priority,
        "progress_review_priority_observed": progress_review_priority,
        "budget": observed_budget if observed_budget is not None else (None, None),
        "pause_observed": budget_pause,
        "progress_review_interval_seconds": interval,
        "slow_worker_delay_ms": slow_worker_delay_ms,
        "due_reviews_observed": len(due_completed),
    }


def _validate_execution(
    value: object,
    roster_size: int,
    receipts: ReceiptStore,
    binding: dict[str, str],
) -> tuple[dict[str, object], dict[str, object]]:
    execution = _object(
        value,
        "execution",
        {
            "machine_invocation_limit",
            "maximum_simultaneous_invocations",
            "priority_policy",
            "budget_policy",
            "timer_traffic",
            "evidence_refs",
        },
    )
    refs = _evidence_refs(
        execution["evidence_refs"],
        "execution.evidence_refs",
        receipts,
        allowed_kinds={"scheduler_trace"},
    )
    if len(refs) != 1:
        raise QualificationError("execution must reference one scheduler trace receipt")
    facts = _scheduler_facts(receipts.documents[refs[0]], binding)
    machine_limit = _integer(
        execution["machine_invocation_limit"],
        "execution.machine_invocation_limit",
        1,
        MAX_ROSTER_SIZE,
    )
    if machine_limit != facts["machine_invocation_limit"]:
        raise QualificationError(
            "execution machine limit summary mismatches its receipt"
        )
    maximum = _integer(
        execution["maximum_simultaneous_invocations"],
        "execution.maximum_simultaneous_invocations",
        0,
        min(machine_limit, roster_size),
    )
    if maximum != facts["maximum_simultaneous_invocations"]:
        raise QualificationError("execution concurrency summary mismatches its receipt")
    priority = _object(
        execution["priority_policy"],
        "execution.priority_policy",
        {
            "default_priority",
            "explicit_equal_priority",
            "equal_priority_order_observed",
            "progress_review_priority_observed",
        },
    )
    _integer(
        priority["default_priority"],
        "execution.priority_policy.default_priority",
        1,
        MAX_PRIORITY,
    )
    _integer(
        priority["explicit_equal_priority"],
        "execution.priority_policy.explicit_equal_priority",
        1,
        MAX_PRIORITY,
    )
    for field in ("equal_priority_order_observed", "progress_review_priority_observed"):
        if not isinstance(priority[field], bool):
            raise QualificationError(
                f"execution.priority_policy.{field} must be boolean"
            )
        if priority[field] is not facts[field]:
            raise QualificationError(
                f"execution.priority_policy.{field} mismatches its scheduler trace"
            )
    if facts["default_priority"] is not None and (
        priority["default_priority"] != facts["default_priority"]
        or priority["explicit_equal_priority"] != facts["explicit_equal_priority"]
    ):
        raise QualificationError(
            "execution priority summary mismatches its scheduler trace"
        )

    budget = _object(
        execution["budget_policy"],
        "execution.budget_policy",
        {"invocation_limit", "active_time_limit_ms", "pause_observed"},
    )
    _optional_positive_integer(
        budget["invocation_limit"], "execution.budget_policy.invocation_limit"
    )
    _optional_positive_integer(
        budget["active_time_limit_ms"],
        "execution.budget_policy.active_time_limit_ms",
    )
    if not isinstance(budget["pause_observed"], bool):
        raise QualificationError(
            "execution.budget_policy.pause_observed must be boolean"
        )
    if budget["pause_observed"] is not facts["pause_observed"]:
        raise QualificationError(
            "execution budget summary mismatches its scheduler trace"
        )
    receipt_budget = facts["budget"]
    if (budget["invocation_limit"], budget["active_time_limit_ms"]) != receipt_budget:
        raise QualificationError("execution budget limits mismatch the scheduler trace")

    timer = _object(
        execution["timer_traffic"],
        "execution.timer_traffic",
        {
            "progress_review_interval_seconds",
            "slow_worker_delay_ms",
            "due_reviews_observed",
        },
    )
    _integer(
        timer["progress_review_interval_seconds"],
        "execution.timer_traffic.progress_review_interval_seconds",
        1,
        86_400,
    )
    _integer(
        timer["slow_worker_delay_ms"],
        "execution.timer_traffic.slow_worker_delay_ms",
        1,
        86_400_000,
    )
    due_reviews = _integer(
        timer["due_reviews_observed"],
        "execution.timer_traffic.due_reviews_observed",
        0,
        1_000_000,
    )
    if (
        timer["progress_review_interval_seconds"]
        != facts["progress_review_interval_seconds"]
        or timer["slow_worker_delay_ms"] != facts["slow_worker_delay_ms"]
        or due_reviews != facts["due_reviews_observed"]
    ):
        raise QualificationError(
            "execution timer summary mismatches its scheduler trace"
        )
    return execution, facts


def _execution_passes(execution: dict[str, object]) -> bool:
    priority = execution["priority_policy"]
    budget = execution["budget_policy"]
    timer = execution["timer_traffic"]
    return bool(
        execution["maximum_simultaneous_invocations"] >= 2
        and priority["default_priority"] == priority["explicit_equal_priority"]
        and priority["equal_priority_order_observed"] is True
        and priority["progress_review_priority_observed"] is True
        and (
            budget["invocation_limit"] is not None
            or budget["active_time_limit_ms"] is not None
        )
        and budget["pause_observed"] is True
        and timer["slow_worker_delay_ms"] > 0
        and timer["due_reviews_observed"] > 0
    )


def _provider_invocation_fact(
    document: dict[str, object],
    binding: dict[str, str],
    provider_name: str,
    label: str,
) -> dict[str, object]:
    receipt = _validate_receipt_binding(
        document,
        label,
        PROVIDER_INVOCATION_SCHEMA,
        binding,
        {
            "provider",
            "cli_version",
            "executable_blake3",
            "member_key",
            "invocation_id",
            "configuration_revision",
            "started_at_ms",
            "finished_at_ms",
            "requested",
            "reported",
            "proposal_schema",
            "coordinator_disposition",
            "submitted_action_id",
            "blocker",
        },
    )
    if receipt["provider"] != provider_name:
        raise QualificationError(f"{label} provider binding mismatch")
    _optional_nonempty(receipt["cli_version"], f"{label}.cli_version")
    executable = _optional_nonempty(
        receipt["executable_blake3"], f"{label}.executable_blake3"
    )
    if executable is not None:
        _blake3(executable, f"{label}.executable_blake3")
    _nonempty(receipt["member_key"], f"{label}.member_key")
    _nonempty(receipt["invocation_id"], f"{label}.invocation_id")
    _integer(
        receipt["configuration_revision"],
        f"{label}.configuration_revision",
        1,
        2**63 - 1,
    )
    started_at = _integer(
        receipt["started_at_ms"], f"{label}.started_at_ms", 0, 2**63 - 1
    )
    finished_at = _integer(
        receipt["finished_at_ms"], f"{label}.finished_at_ms", 0, 2**63 - 1
    )
    if finished_at <= started_at:
        raise QualificationError(f"{label} has a non-positive execution interval")
    requested = _object(
        receipt["requested"], f"{label}.requested", {"model", "effort", "session"}
    )
    _nonempty(requested["model"], f"{label}.requested.model")
    _nonempty(requested["effort"], f"{label}.requested.effort")
    requested_session = _object(
        requested["session"],
        f"{label}.requested.session",
        {"mode", "session_id"},
    )
    if requested_session["mode"] not in {"fresh", "resume"}:
        raise QualificationError(f"{label}.requested.session.mode is invalid")
    requested_session_id = _optional_nonempty(
        requested_session["session_id"], f"{label}.requested.session.session_id"
    )
    if requested_session["mode"] == "resume" and requested_session_id is None:
        raise QualificationError(f"{label} resumes without a session identity")
    reported = receipt["reported"]
    if reported is not None:
        reported = _object(
            reported, f"{label}.reported", {"model", "effort", "session_id"}
        )
        _nonempty(reported["model"], f"{label}.reported.model")
        _nonempty(reported["effort"], f"{label}.reported.effort")
        _nonempty(reported["session_id"], f"{label}.reported.session_id")
    _optional_nonempty(receipt["proposal_schema"], f"{label}.proposal_schema")
    disposition = receipt["coordinator_disposition"]
    if disposition not in {"submitted", "not_submitted"}:
        raise QualificationError(f"{label}.coordinator_disposition is invalid")
    action_id = _optional_nonempty(
        receipt["submitted_action_id"], f"{label}.submitted_action_id"
    )
    blocker = _optional_nonempty(receipt["blocker"], f"{label}.blocker")
    if disposition == "submitted":
        if reported is None or action_id is None or blocker is not None:
            raise QualificationError(f"{label} submitted without complete authority")
    elif action_id is not None or blocker not in {
        "unavailable_setting",
        "configuration_mismatch",
        "provider_unavailable",
        "unreported_configuration",
    }:
        raise QualificationError(
            f"{label} did not retain a safe non-submission blocker"
        )
    return receipt


def _provider_login_fact(
    document: dict[str, object],
    binding: dict[str, str],
    provider_name: str,
    label: str,
) -> dict[str, object]:
    receipt = _validate_receipt_binding(
        document,
        label,
        PROVIDER_LOGIN_SCHEMA,
        binding,
        {
            "provider",
            "cli_version",
            "executable_blake3",
            "login_mode",
            "removed_environment",
            "exit_code",
            "result_sha256",
        },
    )
    if receipt["provider"] != provider_name:
        raise QualificationError(f"{label} provider binding mismatch")
    _optional_nonempty(receipt["cli_version"], f"{label}.cli_version")
    executable = _optional_nonempty(
        receipt["executable_blake3"], f"{label}.executable_blake3"
    )
    if executable is not None:
        _blake3(executable, f"{label}.executable_blake3")
    if receipt["login_mode"] not in {
        "normal_subscription_cli",
        "unavailable",
        "failed",
    }:
        raise QualificationError(f"{label}.login_mode is invalid")
    removed = receipt["removed_environment"]
    if not isinstance(removed, list) or len(removed) > 64:
        raise QualificationError(f"{label}.removed_environment is invalid")
    removed_values = {
        _nonempty(item, f"{label}.removed_environment[]") for item in removed
    }
    if len(removed_values) != len(removed):
        raise QualificationError(f"{label}.removed_environment contains duplicates")
    _integer(receipt["exit_code"], f"{label}.exit_code", 0, 2**31 - 1)
    _digest(receipt["result_sha256"], f"{label}.result_sha256")
    return receipt


def _process_containment_fact(
    document: dict[str, object],
    binding: dict[str, str],
    provider_name: str,
    label: str,
) -> tuple[dict[str, object], dict[str, bool]]:
    receipt = _validate_receipt_binding(
        document,
        label,
        PROCESS_CONTAINMENT_SCHEMA,
        binding,
        {
            "provider",
            "invocation_id",
            "owned_process_ids",
            "delegated_process_ids",
            "escaped_process_ids",
            "terminated_process_ids",
            "cancel_outcome",
            "unrelated_process_id",
            "unrelated_process_status",
        },
    )
    if receipt["provider"] != provider_name:
        raise QualificationError(f"{label} provider binding mismatch")
    _nonempty(receipt["invocation_id"], f"{label}.invocation_id")

    def process_ids(field: str) -> set[str]:
        value = receipt[field]
        if not isinstance(value, list) or len(value) > 1024:
            raise QualificationError(f"{label}.{field} is invalid")
        identifiers = {_nonempty(item, f"{label}.{field}[]") for item in value}
        if len(identifiers) != len(value):
            raise QualificationError(f"{label}.{field} contains duplicates")
        return identifiers

    owned = process_ids("owned_process_ids")
    delegated = process_ids("delegated_process_ids")
    escaped = process_ids("escaped_process_ids")
    terminated = process_ids("terminated_process_ids")
    if receipt["cancel_outcome"] not in {"terminated", "unknown", "not_run"}:
        raise QualificationError(f"{label}.cancel_outcome is invalid")
    _optional_nonempty(receipt["unrelated_process_id"], f"{label}.unrelated_process_id")
    if receipt["unrelated_process_status"] not in {
        "running",
        "terminated",
        "not_observed",
    }:
        raise QualificationError(f"{label}.unrelated_process_status is invalid")
    assertions = {
        # Process ownership proves cleanup, but it does not charge a
        # provider-created subagent to an Agent Swarm roster/provider-cap slot.
        # Until the scheduler has that accounting seam, qualification requires
        # provider-native delegation to be disabled.
        "delegation_contained": not delegated and not escaped,
        "native_cancellation": receipt["cancel_outcome"] == "terminated",
        "owned_descendant_containment": bool(owned)
        and terminated == owned
        and receipt["unrelated_process_status"] == "running",
    }
    return receipt, assertions


def _resource_confinement_fact(
    document: dict[str, object],
    binding: dict[str, str],
    provider_name: str,
    label: str,
) -> tuple[str, str, str, str]:
    receipt = _validate_receipt_binding(
        document,
        label,
        RESOURCE_CONFINEMENT_SCHEMA,
        binding,
        {
            "provider",
            "cli_version",
            "executable_blake3",
            "invocation_id",
            "selection",
            "working_area",
            "resource_policy",
            "allowed_tools",
            "enforcement_layer",
            "in_root_probe",
            "escape_probe",
            "unauthorized_tool_probe",
        },
    )
    if receipt["provider"] != provider_name:
        raise QualificationError(f"{label} provider binding mismatch")
    cli_version = _nonempty(receipt["cli_version"], f"{label}.cli_version")
    executable = _blake3(receipt["executable_blake3"], f"{label}.executable_blake3")
    _nonempty(receipt["invocation_id"], f"{label}.invocation_id")
    selection = _object(receipt["selection"], f"{label}.selection", {"model", "effort"})
    model = _nonempty(selection["model"], f"{label}.selection.model")
    effort = _nonempty(selection["effort"], f"{label}.selection.effort")
    if receipt["resource_policy"] not in {"read_only", "workspace_write"}:
        raise QualificationError(f"{label}.resource_policy is invalid")
    tools = receipt["allowed_tools"]
    if not isinstance(tools, list) or len(tools) > 64:
        raise QualificationError(f"{label}.allowed_tools is invalid")
    allowed_tools = {_nonempty(tool, f"{label}.allowed_tools[]") for tool in tools}
    if len(allowed_tools) != len(tools):
        raise QualificationError(f"{label}.allowed_tools contains duplicates")
    if receipt["enforcement_layer"] != "worldstream-agent-swarm-process-guard":
        raise QualificationError(f"{label}.enforcement_layer is invalid")

    def path_parts(value: object, field: str) -> tuple[str, ...]:
        raw = _nonempty(value, f"{label}.{field}").replace("\\", "/")
        if binding["target"].startswith("windows-"):
            if not re.fullmatch(r"[A-Za-z]:/[^\0]+", raw):
                raise QualificationError(f"{label}.{field} is not absolute")
        elif not raw.startswith("/"):
            raise QualificationError(f"{label}.{field} is not absolute")
        parts = tuple(part for part in raw.split("/") if part)
        if any(part in {".", ".."} for part in parts):
            raise QualificationError(f"{label}.{field} is not normalized")
        return parts

    working = path_parts(receipt["working_area"], "working_area")
    in_root = _object(
        receipt["in_root_probe"], f"{label}.in_root_probe", {"path", "outcome"}
    )
    escape = _object(
        receipt["escape_probe"], f"{label}.escape_probe", {"path", "outcome"}
    )
    tool_probe = _object(
        receipt["unauthorized_tool_probe"],
        f"{label}.unauthorized_tool_probe",
        {"tool", "outcome"},
    )
    in_root_parts = path_parts(in_root["path"], "in_root_probe.path")
    escape_parts = path_parts(escape["path"], "escape_probe.path")
    unauthorized_tool = _nonempty(
        tool_probe["tool"], f"{label}.unauthorized_tool_probe.tool"
    )
    if (
        in_root_parts[: len(working)] != working
        or escape_parts[: len(working)] == working
        or in_root["outcome"] != "allowed"
        or escape["outcome"] != "denied"
        or unauthorized_tool in allowed_tools
        or tool_probe["outcome"] != "denied"
    ):
        raise QualificationError(f"{label} does not prove resource confinement")
    return model, effort, cli_version, executable


def _interval_maximum(receipts: list[dict[str, object]]) -> int:
    boundaries = []
    for receipt in receipts:
        boundaries.append((int(receipt["started_at_ms"]), 1))
        boundaries.append((int(receipt["finished_at_ms"]), -1))
    active = 0
    maximum = 0
    for _, delta in sorted(boundaries, key=lambda item: (item[0], item[1])):
        active += delta
        maximum = max(maximum, active)
    return maximum


def _validate_provider(
    value: object,
    index: int,
    machine_limit: int,
    receipts: ReceiptStore,
    binding: dict[str, str],
    scheduler_facts: dict[str, object],
) -> dict[str, object]:
    provider = _object(
        value,
        f"providers[{index}]",
        {
            "provider",
            "status",
            "reason",
            "evidence_refs",
            "cli_version",
            "executable_blake3",
            "explicit_model",
            "explicit_effort",
            "session_reuse",
            "member_count",
            "effective_cap",
            "maximum_simultaneous_invocations",
            "selections",
            "assertions",
        },
    )
    name = provider["provider"]
    if name not in PROVIDERS:
        raise QualificationError("provider evidence names an unsupported provider")
    if provider["status"] not in VERDICTS:
        raise QualificationError(f"{name}.status is invalid")
    _nonempty(provider["reason"], f"{name}.reason")
    refs = _evidence_refs(
        provider["evidence_refs"],
        f"{name}.evidence_refs",
        receipts,
        allowed_kinds={
            "provider_invocation",
            "provider_login",
            "process_containment",
            "resource_confinement",
        },
    )
    invocation_receipts = [
        _provider_invocation_fact(
            receipts.documents[receipt_id], binding, str(name), f"receipt {receipt_id}"
        )
        for receipt_id in refs
        if receipts.entries[receipt_id]["kind"] == "provider_invocation"
    ]
    login_refs = [
        receipt_id
        for receipt_id in refs
        if receipts.entries[receipt_id]["kind"] == "provider_login"
    ]
    process_refs = [
        receipt_id
        for receipt_id in refs
        if receipts.entries[receipt_id]["kind"] == "process_containment"
    ]
    confinement_refs = [
        receipt_id
        for receipt_id in refs
        if receipts.entries[receipt_id]["kind"] == "resource_confinement"
    ]
    if not invocation_receipts or len(login_refs) != 1 or len(process_refs) != 1:
        raise QualificationError(
            f"{name} must reference invocations, one login, and one containment receipt"
        )
    login = _provider_login_fact(
        receipts.documents[login_refs[0]],
        binding,
        str(name),
        f"receipt {login_refs[0]}",
    )
    _, process_assertions = _process_containment_fact(
        receipts.documents[process_refs[0]],
        binding,
        str(name),
        f"receipt {process_refs[0]}",
    )
    confinement_facts = [
        _resource_confinement_fact(
            receipts.documents[receipt_id],
            binding,
            str(name),
            f"receipt {receipt_id}",
        )
        for receipt_id in confinement_refs
    ]
    confined_selections = {(item[0], item[1]) for item in confinement_facts}
    cli_version = _optional_nonempty(provider["cli_version"], f"{name}.cli_version")
    executable_digest = _optional_nonempty(
        provider["executable_blake3"], f"{name}.executable_blake3"
    )
    if executable_digest is not None:
        _blake3(executable_digest, f"{name}.executable_blake3")
    for field in ("explicit_model", "explicit_effort"):
        if not isinstance(provider[field], bool):
            raise QualificationError(f"{name}.{field} must be boolean")
    if provider["session_reuse"] is not None and not isinstance(
        provider["session_reuse"], bool
    ):
        raise QualificationError(f"{name}.session_reuse must be boolean or null")
    member_count = _integer(
        provider["member_count"], f"{name}.member_count", 2, MAX_ROSTER_SIZE
    )
    effective_cap = _integer(
        provider["effective_cap"], f"{name}.effective_cap", 0, MAX_ROSTER_SIZE
    )
    maximum = _integer(
        provider["maximum_simultaneous_invocations"],
        f"{name}.maximum_simultaneous_invocations",
        0,
        min(machine_limit, effective_cap),
    )
    selections = provider["selections"]
    if not isinstance(selections, list) or len(selections) > MAX_ROSTER_SIZE:
        raise QualificationError(f"{name}.selections must be a bounded array")
    distinct: set[tuple[str, str]] = set()
    for selection in selections:
        selection = _object(selection, f"{name}.selection", {"model", "effort"})
        model = _nonempty(selection["model"], f"{name}.model")
        effort = _nonempty(selection["effort"], f"{name}.effort")
        distinct.add((model, effort))
    assertions = provider["assertions"]
    if not isinstance(assertions, dict) or set(assertions) != PROVIDER_ASSERTIONS:
        raise QualificationError(f"{name} provider assertions are incomplete")
    if any(not isinstance(assertions[key], bool) for key in PROVIDER_ASSERTIONS):
        raise QualificationError(f"{name} provider assertions must be boolean")

    receipt_cli_versions = {
        item["cli_version"]
        for item in [login, *invocation_receipts]
        if item["cli_version"] is not None
    }
    receipt_cli_versions.update(item[2] for item in confinement_facts)
    receipt_executables = {
        item["executable_blake3"]
        for item in [login, *invocation_receipts]
        if item["executable_blake3"] is not None
    }
    receipt_executables.update(item[3] for item in confinement_facts)
    derived_cli_version = (
        next(iter(receipt_cli_versions)) if len(receipt_cli_versions) == 1 else None
    )
    derived_executable = (
        next(iter(receipt_executables)) if len(receipt_executables) == 1 else None
    )
    if len(receipt_cli_versions) > 1 or len(receipt_executables) > 1:
        raise QualificationError(f"{name} receipts disagree on executable identity")
    if cli_version != derived_cli_version or executable_digest != derived_executable:
        raise QualificationError(f"{name} executable summary mismatches its receipts")

    members = {str(item["member_key"]) for item in invocation_receipts}
    if len(members) != member_count:
        raise QualificationError(f"{name} member count is not receipt-derived")
    accepted = [
        item
        for item in invocation_receipts
        if item["coordinator_disposition"] == "submitted"
    ]
    accepted_exact = [
        item
        for item in accepted
        if item["reported"] is not None
        and item["reported"]["model"] == item["requested"]["model"]
        and item["reported"]["effort"] == item["requested"]["effort"]
        and (
            (
                item["requested"]["session"]["mode"] == "fresh"
                and item["requested"]["session"]["session_id"] is None
                and item["reported"]["session_id"] is not None
            )
            or item["reported"]["session_id"]
            == item["requested"]["session"]["session_id"]
        )
    ]
    derived_selections = {
        (str(item["requested"]["model"]), str(item["requested"]["effort"]))
        for item in accepted_exact
    }
    if distinct != derived_selections:
        raise QualificationError(
            f"{name} selection summary mismatches invocation receipts"
        )

    isolation = any(
        left["member_key"] != right["member_key"]
        and (left["requested"]["model"], left["requested"]["effort"])
        != (right["requested"]["model"], right["requested"]["effort"])
        and int(left["started_at_ms"]) < int(right["finished_at_ms"])
        and int(right["started_at_ms"]) < int(left["finished_at_ms"])
        for offset, left in enumerate(accepted_exact)
        for right in accepted_exact[offset + 1 :]
    )
    next_turn = False
    derived_session_reuse = False
    by_member: dict[str, list[dict[str, object]]] = {}
    for item in accepted_exact:
        by_member.setdefault(str(item["member_key"]), []).append(item)
    for member_receipts in by_member.values():
        ordered = sorted(
            member_receipts, key=lambda item: int(item["configuration_revision"])
        )
        for previous, current in pairwise(ordered):
            resumed_previous = bool(
                current["requested"]["session"]["mode"] == "resume"
                and current["requested"]["session"]["session_id"]
                == previous["reported"]["session_id"]
            )
            derived_session_reuse = derived_session_reuse or resumed_previous
            changed = (
                previous["requested"]["model"],
                previous["requested"]["effort"],
            ) != (current["requested"]["model"], current["requested"]["effort"])
            if (
                int(current["configuration_revision"])
                == int(previous["configuration_revision"]) + 1
                and int(current["started_at_ms"]) >= int(previous["finished_at_ms"])
                and changed
            ):
                next_turn = True
    blocked_setting = any(
        item["coordinator_disposition"] == "not_submitted"
        and item["submitted_action_id"] is None
        and (
            item["reported"] is None
            or item["reported"]["model"] != item["requested"]["model"]
            or item["reported"]["effort"] != item["requested"]["effort"]
        )
        for item in invocation_receipts
    )
    subscription_login = bool(
        login["login_mode"] == "normal_subscription_cli"
        and login["exit_code"] == 0
        and DIRECT_API_ENVIRONMENT.issubset(set(login["removed_environment"]))
    )
    derived_assertions = {
        "subscription_login": subscription_login,
        "deterministic_contract": bool(accepted_exact)
        and all(
            item["proposal_schema"] == "worldstream/agent-swarm-action-proposal@1"
            for item in accepted_exact
        ),
        "effective_configuration_report": bool(accepted)
        and len(accepted) == len(accepted_exact),
        **process_assertions,
        "resource_confinement": confined_selections == derived_selections,
        "same_provider_setting_isolation": isolation,
        "next_invocation_setting_change": next_turn,
        "unavailable_or_substituted_setting_blocks": blocked_setting,
    }
    if assertions != derived_assertions:
        raise QualificationError(f"{name} assertion summary mismatches typed receipts")
    if provider["explicit_model"] is not bool(derived_selections) or provider[
        "explicit_effort"
    ] is not bool(derived_selections):
        raise QualificationError(f"{name} explicit setting summary mismatches receipts")
    if provider["session_reuse"] is not derived_session_reuse:
        raise QualificationError(f"{name} session summary mismatches receipts")
    scheduler_intervals = {
        invocation_id: interval
        for invocation_id, interval in scheduler_facts["intervals"].items()
        if interval[0] == name
    }
    receipt_intervals = {
        str(item["invocation_id"]): (
            name,
            int(item["started_at_ms"]),
            int(item["finished_at_ms"]),
        )
        for item in accepted_exact
    }
    if scheduler_intervals != receipt_intervals:
        raise QualificationError(
            f"{name} invocation intervals mismatch scheduler trace"
        )
    receipt_maximum = _interval_maximum(accepted_exact)
    if (
        maximum != receipt_maximum
        or maximum != scheduler_facts["provider_maximum"][name]
    ):
        raise QualificationError(
            f"{name} concurrency summary mismatches typed receipts"
        )
    if effective_cap != scheduler_facts["provider_caps"][name]:
        raise QualificationError(f"{name} effective cap mismatches scheduler trace")

    qualified = bool(
        cli_version is not None
        and executable_digest is not None
        and provider["explicit_model"] is True
        and provider["explicit_effort"] is True
        and isinstance(provider["session_reuse"], bool)
        and len(distinct) >= 2
        and all(derived_assertions[key] is True for key in PROVIDER_ASSERTIONS)
        and effective_cap > 0
        and maximum > 0
    )
    if (provider["status"] == "pass") != qualified:
        raise QualificationError(
            f"{name}.status contradicts its retained provider evidence"
        )
    return provider


def _validate_result_lineage(value: object, label: str) -> dict[str, object]:
    lineage = _object(
        value,
        label,
        {
            "accepted_result_id",
            "candidate_id",
            "review_id",
            "producer_member_key",
            "reviewer_member_key",
            "artifact_sha256",
        },
    )
    for field in (
        "accepted_result_id",
        "candidate_id",
        "review_id",
        "producer_member_key",
        "reviewer_member_key",
    ):
        _nonempty(lineage[field], f"{label}.{field}")
    _digest(lineage["artifact_sha256"], f"{label}.artifact_sha256")
    if lineage["producer_member_key"] == lineage["reviewer_member_key"]:
        raise QualificationError(f"{label} does not prove independent review")
    return lineage


def _validate_scenario(
    name: str,
    value: object,
    artifacts: dict[str, object],
    receipts: ReceiptStore,
    binding: dict[str, str],
) -> dict[str, object]:
    scenario = _object(
        value,
        f"scenarios.{name}",
        {
            "verdict",
            "reason",
            "evidence_refs",
            "engine_instance_id",
            "goal_ref",
            "room_id",
            "swarm_id",
            "criteria_revision",
            "head",
            "action_id",
            "result_lineage",
        },
    )
    if scenario["verdict"] not in VERDICTS:
        raise QualificationError(f"scenarios.{name}.verdict is invalid")
    _nonempty(scenario["reason"], f"scenarios.{name}.reason")
    refs = _evidence_refs(
        scenario["evidence_refs"],
        f"scenarios.{name}.evidence_refs",
        receipts,
        allowed_kinds={"scenario"},
    )
    if len(refs) != 1:
        raise QualificationError(
            f"scenarios.{name} must reference one scenario receipt"
        )
    receipt_id = refs[0]
    retained = _validate_receipt_binding(
        receipts.documents[receipt_id],
        f"receipt {receipt_id}",
        SCENARIO_RECEIPT_SCHEMA,
        binding,
        {
            "scenario",
            "verdict",
            "reason",
            "goal_ref",
            "room_id",
            "swarm_id",
            "criteria_revision",
            "head",
            "action_id",
            "result_lineage",
            "source_receipt_ids",
        },
    )
    if retained["scenario"] != name:
        raise QualificationError(f"receipt {receipt_id} scenario binding mismatch")
    source_refs = receipts.references(
        retained["source_receipt_ids"],
        f"receipt {receipt_id}.source_receipt_ids",
        allowed_kinds={
            "provider_invocation",
            "provider_login",
            "scheduler_trace",
            "process_containment",
        },
    )
    source_kinds = {receipts.entries[item]["kind"] for item in source_refs}
    required_source_kind = (
        "provider_invocation"
        if name
        in {
            "sourced_report",
            "reviewed_code_change",
            "explicit_provider_settings",
            "unavailable_settings_blocked",
        }
        else "process_containment"
        if name == "owned_descendant_cleanup"
        else "scheduler_trace"
    )
    if scenario["verdict"] == "pass" and required_source_kind not in source_kinds:
        raise QualificationError(
            f"receipt {receipt_id} lacks a {required_source_kind} source"
        )
    for field in (
        "verdict",
        "reason",
        "goal_ref",
        "room_id",
        "swarm_id",
        "criteria_revision",
        "head",
        "action_id",
        "result_lineage",
    ):
        if scenario[field] != retained[field]:
            raise QualificationError(
                f"scenarios.{name}.{field} summary mismatches its receipt"
            )
    if scenario["engine_instance_id"] != binding["engine_instance_id"]:
        raise QualificationError(
            f"scenarios.{name} was not produced by the retained coordination engine"
        )
    fields = ("engine_instance_id", "goal_ref", "room_id", "swarm_id")
    if scenario["verdict"] == "pass":
        for field in fields:
            _nonempty(scenario[field], f"scenarios.{name}.{field}")
        _integer(
            scenario["criteria_revision"],
            f"scenarios.{name}.criteria_revision",
            1,
            2**63 - 1,
        )
        _nonempty(scenario["action_id"], f"scenarios.{name}.action_id")
        head = _object(
            scenario["head"],
            f"scenarios.{name}.head",
            {"room_seq", "authoritative_state_hash"},
        )
        _integer(head["room_seq"], f"scenarios.{name}.head.room_seq", 0, 2**63 - 1)
        _blake3(
            head["authoritative_state_hash"],
            f"scenarios.{name}.head.authoritative_state_hash",
        )
    else:
        for field in fields:
            _optional_nonempty(scenario[field], f"scenarios.{name}.{field}")
        if scenario["head"] is not None:
            head = _object(
                scenario["head"],
                f"scenarios.{name}.head",
                {"room_seq", "authoritative_state_hash"},
            )
            _integer(head["room_seq"], f"scenarios.{name}.head.room_seq", 0, 2**63 - 1)
            _blake3(
                head["authoritative_state_hash"],
                f"scenarios.{name}.head.authoritative_state_hash",
            )
        if scenario["criteria_revision"] is not None:
            _integer(
                scenario["criteria_revision"],
                f"scenarios.{name}.criteria_revision",
                1,
                2**63 - 1,
            )
        _optional_nonempty(scenario["action_id"], f"scenarios.{name}.action_id")

    lineage_value = scenario["result_lineage"]
    if lineage_value is not None:
        lineage = _validate_result_lineage(
            lineage_value, f"scenarios.{name}.result_lineage"
        )
    else:
        lineage = None
    artifact_name = RESULT_SCENARIOS.get(name)
    if scenario["verdict"] == "pass" and artifact_name is not None:
        artifact = artifacts[artifact_name]
        if not isinstance(artifact, dict) or lineage is None:
            raise QualificationError(f"scenarios.{name} lacks accepted Result lineage")
        if (
            lineage["accepted_result_id"] != artifact["accepted_result_id"]
            or lineage["artifact_sha256"] != artifact["sha256"]
        ):
            raise QualificationError(
                f"scenarios.{name} Result lineage does not bind its artifact"
            )
    return scenario


def _validate_artifacts(value: object) -> dict[str, object]:
    artifacts = _object(value, "artifacts", {"report", "code_change"})
    for name, artifact in artifacts.items():
        if artifact is None:
            continue
        artifact = _object(
            artifact, f"artifacts.{name}", {"sha256", "accepted_result_id"}
        )
        _digest(artifact["sha256"], f"artifacts.{name}.sha256")
        _nonempty(
            artifact["accepted_result_id"], f"artifacts.{name}.accepted_result_id"
        )
    return artifacts


def load_evidence(path: Path) -> tuple[dict[str, object], str]:
    """Load one native target record without discarding failed support rows."""

    raw, value = _read_bounded_regular_json(path, "evidence")
    document = _object(
        value,
        "evidence",
        {
            "schema",
            "run_id",
            "recorded_at",
            "git_revision",
            "receipt_manifest",
            "package",
            "platform",
            "coordination_engine",
            "roster_size",
            "execution",
            "providers",
            "scenarios",
            "artifacts",
            "status",
        },
    )
    if document["schema"] != EVIDENCE_SCHEMA:
        raise QualificationError(f"evidence is not an {EVIDENCE_SCHEMA} document")
    if document["status"] not in VERDICTS:
        raise QualificationError("evidence status is invalid")
    _nonempty(document["run_id"], "run_id")
    _nonempty(document["recorded_at"], "recorded_at")
    revision = _nonempty(document["git_revision"], "git_revision")
    if not GIT_REVISION.fullmatch(revision):
        raise QualificationError("git_revision must be a lowercase full revision")

    platform = _object(document["platform"], "platform", {"os", "arch", "native"})
    if platform["native"] is not True:
        raise QualificationError("evidence must come from a native target")
    receipts = ReceiptStore(document["receipt_manifest"], path)
    package = _validate_package(document["package"], platform, receipts)
    engine = _object(
        document["coordination_engine"],
        "coordination_engine",
        {"application", "instance_id", "pack_digest"},
    )
    if engine["application"] != COORDINATION_ENGINE:
        raise QualificationError("coordination engine identity mismatch")
    engine_instance_id = _nonempty(
        engine["instance_id"], "coordination_engine.instance_id"
    )
    if engine["pack_digest"] != package["pack_digest"]:
        raise QualificationError("coordination engine Pack identity mismatch")
    binding = {
        "run_id": str(document["run_id"]),
        "target": str(package["target"]),
        "package_sha256": str(package["sha256"]),
        "install_sha256": str(package["install_sha256"]),
        "engine_instance_id": engine_instance_id,
    }

    roster_size = _integer(
        document["roster_size"], "roster_size", MIN_ROSTER_SIZE, MAX_ROSTER_SIZE
    )
    execution, scheduler_facts = _validate_execution(
        document["execution"], roster_size, receipts, binding
    )
    providers = document["providers"]
    if not isinstance(providers, list) or len(providers) != len(PROVIDERS):
        raise QualificationError("evidence must contain Codex, Claude, and Kiro")
    seen: set[str] = set()
    provider_members = 0
    validated_providers = []
    for index, provider_value in enumerate(providers):
        provider = _validate_provider(
            provider_value,
            index,
            int(execution["machine_invocation_limit"]),
            receipts,
            binding,
            scheduler_facts,
        )
        name = str(provider["provider"])
        if name in seen:
            raise QualificationError("provider evidence is duplicated")
        seen.add(name)
        provider_members += int(provider["member_count"])
        validated_providers.append(provider)
    if seen != PROVIDERS:
        raise QualificationError("provider evidence is incomplete")
    if provider_members != roster_size:
        raise QualificationError("provider member counts do not match the roster size")

    artifacts = _validate_artifacts(document["artifacts"])
    scenarios = document["scenarios"]
    if not isinstance(scenarios, dict) or set(scenarios) != SCENARIOS:
        raise QualificationError("scenario evidence set is incomplete")
    validated_scenarios = {
        name: _validate_scenario(name, scenarios[name], artifacts, receipts, binding)
        for name in sorted(SCENARIOS)
    }
    qualified = bool(
        _execution_passes(execution)
        and all(provider["status"] == "pass" for provider in validated_providers)
        and all(
            scenario["verdict"] == "pass" for scenario in validated_scenarios.values()
        )
        and all(artifacts[name] is not None for name in RESULT_SCENARIOS.values())
    )
    if (document["status"] == "pass") != qualified:
        raise QualificationError("evidence status contradicts its retained facts")
    receipts.ensure_all_used()
    return document, hashlib.sha256(raw).hexdigest()


def _missing_support(target: str, provider: str) -> dict[str, object]:
    operating_system, architecture = ADVERTISED_TARGETS[target]
    return {
        "target": target,
        "os": operating_system,
        "arch": architecture,
        "provider": provider,
        "status": "missing",
        "reason": "no native target evidence supplied",
        "evidence_refs": [],
        "cli_version": None,
        "executable_blake3": None,
        "explicit_model": False,
        "explicit_effort": False,
        "session_reuse": None,
        "member_count": None,
        "effective_cap": None,
        "maximum_simultaneous_invocations": None,
        "selections": [],
        "assertions": None,
    }


def _missing_scenario(target: str, scenario: str) -> dict[str, object]:
    return {
        "target": target,
        "scenario": scenario,
        "verdict": "missing",
        "reason": "no native target evidence supplied",
        "evidence_refs": [],
        "engine_instance_id": None,
        "goal_ref": None,
        "room_id": None,
        "swarm_id": None,
        "criteria_revision": None,
        "head": None,
        "action_id": None,
        "result_lineage": None,
    }


def assemble(paths: list[Path], expected_revision: str) -> dict[str, object]:
    """Assemble a complete advertised-target matrix and a fail-closed verdict."""

    if not paths:
        raise QualificationError("no native qualification evidence was supplied")
    if not GIT_REVISION.fullmatch(expected_revision):
        raise QualificationError("expected revision must be a lowercase full revision")
    by_target: dict[str, tuple[dict[str, object], str, Path]] = {}
    run_ids: set[str] = set()
    application_identity: tuple[str, str, str, str] | None = None
    for path in paths:
        document, evidence_digest = load_evidence(path)
        if document["git_revision"] != expected_revision:
            raise QualificationError(f"git revision mismatch in {path}")
        target = str(document["package"]["target"])
        if target in by_target:
            raise QualificationError(f"duplicate {target} evidence")
        run_id = str(document["run_id"])
        if run_id in run_ids:
            raise QualificationError("native evidence run_id is duplicated")
        run_ids.add(run_id)
        package = document["package"]
        identity = (
            str(package["application_version"]),
            str(package["install_schema"]),
            str(package["pack_version"]),
            str(package["pack_digest"]),
        )
        if application_identity is None:
            application_identity = identity
        elif identity != application_identity:
            raise QualificationError("native target application/Pack identity mismatch")
        by_target[target] = (document, evidence_digest, path)

    missing_targets = sorted(set(ADVERTISED_TARGETS) - set(by_target))
    support_matrix: list[dict[str, object]] = []
    scenario_matrix: list[dict[str, object]] = []
    gaps: list[str] = []
    for target in sorted(ADVERTISED_TARGETS):
        retained = by_target.get(target)
        if retained is None:
            gaps.append(f"{target}: missing native evidence")
            support_matrix.extend(
                _missing_support(target, provider) for provider in sorted(PROVIDERS)
            )
            scenario_matrix.extend(
                _missing_scenario(target, scenario) for scenario in sorted(SCENARIOS)
            )
            continue
        document = retained[0]
        operating_system, architecture = ADVERTISED_TARGETS[target]
        for provider in sorted(
            document["providers"], key=lambda item: item["provider"]
        ):
            row = {
                "target": target,
                "os": operating_system,
                "arch": architecture,
                **{
                    key: provider[key]
                    for key in (
                        "provider",
                        "status",
                        "reason",
                        "evidence_refs",
                        "cli_version",
                        "executable_blake3",
                        "explicit_model",
                        "explicit_effort",
                        "session_reuse",
                        "member_count",
                        "effective_cap",
                        "maximum_simultaneous_invocations",
                        "selections",
                        "assertions",
                    )
                },
            }
            support_matrix.append(row)
            if provider["status"] != "pass":
                gaps.append(
                    f"{target}/{provider['provider']}: {provider['status']} — {provider['reason']}"
                )
        for scenario_name in sorted(SCENARIOS):
            scenario = document["scenarios"][scenario_name]
            scenario_matrix.append(
                {"target": target, "scenario": scenario_name, **scenario}
            )
            if scenario["verdict"] != "pass":
                gaps.append(
                    f"{target}/{scenario_name}: {scenario['verdict']} — {scenario['reason']}"
                )
        if document["status"] != "pass" and not any(
            gap.startswith(f"{target}/") for gap in gaps
        ):
            gaps.append(f"{target}: native run status is {document['status']}")

    ready = not missing_targets and all(
        retained[0]["status"] == "pass" for retained in by_target.values()
    )
    return {
        "schema": REPORT_SCHEMA,
        "status": "pass" if ready else "fail",
        "git_revision": expected_revision,
        "advertised_targets": sorted(ADVERTISED_TARGETS),
        "missing_targets": missing_targets,
        "application_identity": (
            {
                "application_version": application_identity[0],
                "install_schema": application_identity[1],
                "pack_version": application_identity[2],
                "pack_digest": application_identity[3],
            }
            if application_identity is not None
            else None
        ),
        "packages": {
            target: retained[0]["package"]
            for target, retained in sorted(by_target.items())
        },
        "native_targets": [
            {
                "target": target,
                "os": retained[0]["platform"]["os"],
                "arch": retained[0]["platform"]["arch"],
                "status": retained[0]["status"],
                "run_id": retained[0]["run_id"],
                "evidence_sha256": retained[1],
                "evidence_file": retained[2].name,
                "roster_size": retained[0]["roster_size"],
                "execution": retained[0]["execution"],
            }
            for target, retained in sorted(by_target.items())
        ],
        "providers": sorted(PROVIDERS),
        "support_matrix": support_matrix,
        "scenario_matrix": scenario_matrix,
        "qualification_gaps": gaps,
    }


def provider_qualification_records(
    paths: list[Path], expected_revision: str
) -> dict[str, dict[str, object]]:
    """Derive importable exact-executable records from a passing full matrix."""

    report = assemble(paths, expected_revision)
    if report["status"] != "pass":
        raise QualificationError(
            "provider qualification records require a passing complete target matrix"
        )
    records: dict[str, dict[str, object]] = {}
    for path in paths:
        document, evidence_sha256 = load_evidence(path)
        platform = document["platform"]
        for provider in document["providers"]:
            key = f"{platform['os']}-{platform['arch']}-{provider['provider']}"
            records[key] = {
                "schema": PROVIDER_QUALIFICATION_SCHEMA,
                "provider": provider["provider"],
                "executable_digest": provider["executable_blake3"],
                "version": provider["cli_version"],
                "operating_system": platform["os"],
                "architecture": platform["arch"],
                "evidence_sha256": evidence_sha256,
                "evidence_path": f"{platform['os']}-{platform['arch']}/evidence.json",
                "receipt_manifest_sha256": receipt_manifest_sha256(
                    document["receipt_manifest"]
                ),
                "subscription_login": provider["assertions"]["subscription_login"],
                "explicit_model": provider["explicit_model"],
                "explicit_effort": provider["explicit_effort"],
                "session_reuse": provider["session_reuse"],
                "reports_effective_configuration": provider["assertions"][
                    "effective_configuration_report"
                ],
                "delegation_contained": provider["assertions"]["delegation_contained"],
                "native_cancellation_qualified": provider["assertions"][
                    "native_cancellation"
                ]
                and provider["assertions"]["owned_descendant_containment"],
                "resource_confinement_qualified": provider["assertions"][
                    "resource_confinement"
                ],
                "selections": provider["selections"],
            }
    return records


def _write_qualification_evidence_bundle(paths: list[Path], output: Path) -> None:
    """Copy each already-validated evidence graph into a self-contained bundle."""

    for source in paths:
        document, _ = load_evidence(source)
        platform = document["platform"]
        target_directory = output / f"{platform['os']}-{platform['arch']}"
        target_directory.mkdir(mode=0o700)
        evidence_destination = target_directory / "evidence.json"
        evidence_destination.write_bytes(source.read_bytes())
        evidence_destination.chmod(0o600)
        for entry in document["receipt_manifest"].values():
            relative = PurePosixPath(entry["path"])
            source_receipt = source.parent.joinpath(*relative.parts)
            destination = target_directory.joinpath(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            destination.write_bytes(source_receipt.read_bytes())
            destination.chmod(0o600)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, action="append", required=True)
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--provider-qualification-dir", type=Path)
    args = parser.parse_args()
    try:
        report = assemble(args.evidence, args.expected_revision)
        if args.output.exists():
            raise QualificationError(f"refusing to overwrite report: {args.output}")
        if (
            args.provider_qualification_dir is not None
            and args.provider_qualification_dir.exists()
        ):
            raise QualificationError(
                f"refusing to overwrite qualification directory: {args.provider_qualification_dir}"
            )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(canonical_json(report))
        if report["status"] != "pass":
            print(
                f"agent-swarm qualification incomplete; retained failure report: {args.output}",
                file=os.sys.stderr,
            )
            return 1
        if args.provider_qualification_dir is not None:
            records = provider_qualification_records(
                args.evidence, args.expected_revision
            )
            args.provider_qualification_dir.mkdir(parents=True, mode=0o700)
            _write_qualification_evidence_bundle(
                args.evidence, args.provider_qualification_dir
            )
            for name, record in sorted(records.items()):
                destination = args.provider_qualification_dir / f"{name}.json"
                destination.write_bytes(canonical_json(record))
                destination.chmod(0o600)
        print(args.output)
    except QualificationError as error:
        print(f"agent-swarm qualification incomplete: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
