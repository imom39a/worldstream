#!/usr/bin/env python3
"""Validate WorldStream's outside-adopter qualification receipts.

This tool never creates a passing receipt and never treats repository tests as
an outside-adopter trial. It validates bounded, redacted receipts produced by
non-contributors using one exact released distribution, then emits a canonical
qualification summary suitable for later detached release evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA = "worldstream/outside-adopter-trial/v1"
SUMMARY_SCHEMA = "worldstream/outside-adopter-qualification/v1"
MAX_RECEIPT_BYTES = 256 * 1024
MAX_RECEIPTS = 32
MAX_ROWS = 128
MAX_TEXT_BYTES = 2048
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
TRIAL_ID = re.compile(r"trial_[0-9a-z-]{8,64}\Z")
SAFE_CODE = re.compile(r"[a-z][a-z0-9_-]{0,127}\Z")
JOURNEYS = frozenset({"pack_author", "application_integrator"})
PACK_ROUTES = frozenset({"deterministic", "prompt_assisted"})
INTEGRATOR_ROUTE = "official_negotiate"
PROFILES = frozenset(
    {
        "native-linux-x86_64",
        "native-windows-x64",
        "oci-linux-amd64",
    }
)
REQUIRED_ARTIFACTS = frozenset({"release_manifest", "starter_distribution"})
PACK_CHECKPOINTS = frozenset(
    {"scaffold", "check", "test", "build", "inspect", "prove", "real_room"}
)
PROMPT_CHECKPOINT = "prompt_scaffold"
INTEGRATOR_CHECKPOINTS = frozenset(
    {
        "install",
        "approve",
        "restart_readiness",
        "create_room",
        "attach_independent_agents",
        "forced_reconnect",
        "replay",
        "evidence_verify",
    }
)


class TrialError(RuntimeError):
    """A closed validation failure safe to report to the operator."""


@dataclass(frozen=True)
class Trial:
    trial_id: str
    participant_id: str
    journey: str
    route: str
    profile: str
    elapsed_seconds: int
    receipt_sha256: str


def fail(message: str) -> None:
    raise TrialError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def strict_json(content: bytes, label: str) -> dict[str, Any]:
    def no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                fail(f"{label} contains duplicate JSON key {key!r}")
            value[key] = item
        return value

    try:
        value = json.loads(
            content,
            object_pairs_hook=no_duplicates,
            parse_constant=lambda constant: fail(
                f"{label} contains non-JSON numeric constant {constant!r}"
            ),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise TrialError(f"{label} is not strict JSON") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def regular_bytes(path: Path, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise TrialError(f"{label} is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > MAX_RECEIPT_BYTES:
        fail(f"{label} is outside the bounded size limit")
    try:
        content = path.read_bytes()
        finished = path.lstat()
    except OSError as error:
        raise TrialError(f"{label} could not be read") from error
    before = (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns)
    after = (finished.st_dev, finished.st_ino, finished.st_size, finished.st_mtime_ns)
    if before != after or len(content) != metadata.st_size:
        fail(f"{label} changed while it was read")
    return content


def validate_receipt(path: Path, expected_release_manifest_sha256: str) -> Trial:
    content = regular_bytes(path, f"trial receipt {path.name}")
    value = strict_json(content, f"trial receipt {path.name}")
    expected_fields = {
        "schema",
        "trial_id",
        "participant",
        "journey",
        "route",
        "installation_profile",
        "release_manifest_sha256",
        "started_at",
        "completed_at",
        "elapsed_seconds",
        "outcome",
        "constraints",
        "checkpoints",
        "artifacts",
        "commands",
        "diagnostics",
    }
    if set(value) != expected_fields:
        fail(f"trial receipt {path.name} has an unexpected field inventory")
    if value["schema"] != SCHEMA:
        fail(f"trial receipt {path.name} has the wrong schema")
    trial_id = value["trial_id"]
    if not isinstance(trial_id, str) or TRIAL_ID.fullmatch(trial_id) is None:
        fail(f"trial receipt {path.name} has an invalid trial_id")
    participant = object_value(value["participant"], "participant")
    if set(participant) != {"participant_id", "non_contributor", "prior_contribution"}:
        fail(f"trial receipt {path.name} has an invalid participant witness")
    participant_id = bounded_code(participant["participant_id"], "participant_id")
    if (
        participant["non_contributor"] is not True
        or participant["prior_contribution"] is not False
    ):
        fail(f"trial receipt {path.name} is not an outside-adopter witness")
    journey = value["journey"]
    route = value["route"]
    if journey not in JOURNEYS:
        fail(f"trial receipt {path.name} has an unsupported journey")
    if journey == "pack_author" and route not in PACK_ROUTES:
        fail(f"trial receipt {path.name} has an unsupported Pack Author route")
    if journey == "application_integrator" and route != INTEGRATOR_ROUTE:
        fail(f"trial receipt {path.name} has an unsupported integrator route")
    profile = value["installation_profile"]
    if profile not in PROFILES:
        fail(f"trial receipt {path.name} has an unsupported installation profile")
    if value["release_manifest_sha256"] != expected_release_manifest_sha256:
        fail(f"trial receipt {path.name} used a different release manifest")

    started = utc_second(value["started_at"], "started_at")
    completed = utc_second(value["completed_at"], "completed_at")
    elapsed = value["elapsed_seconds"]
    if not isinstance(elapsed, int) or isinstance(elapsed, bool) or elapsed < 0:
        fail(f"trial receipt {path.name} has an invalid elapsed_seconds")
    observed_elapsed = int((completed - started).total_seconds())
    if observed_elapsed != elapsed:
        fail(f"trial receipt {path.name} elapsed time disagrees with its timestamps")
    limit = 3600 if journey == "pack_author" else 1800
    if elapsed > limit:
        fail(f"trial receipt {path.name} exceeded its {limit}-second limit")
    if value["outcome"] != "passed":
        fail(f"trial receipt {path.name} did not pass")

    constraints = object_value(value["constraints"], "constraints")
    required_constraints = {
        "clean_environment": True,
        "released_artifacts_only": True,
        "source_or_adr_access": False,
        "linear_or_chat_access": False,
        "rust_or_runtime_rebuild": False,
        "manual_database_access": False,
        "paid_dependency": False,
        "maintainer_interventions": 0,
    }
    if constraints != required_constraints:
        fail(f"trial receipt {path.name} violates the outside-adopter constraints")

    checkpoints = string_set(value["checkpoints"], "checkpoints")
    expected_checkpoints = (
        PACK_CHECKPOINTS
        | ({PROMPT_CHECKPOINT} if route == "prompt_assisted" else set())
        if journey == "pack_author"
        else INTEGRATOR_CHECKPOINTS
    )
    if checkpoints != expected_checkpoints:
        fail(f"trial receipt {path.name} has an incomplete checkpoint inventory")
    artifacts = validate_rows(
        value["artifacts"], "artifacts", {"name", "sha256", "size_bytes"}
    )
    if [artifact["name"] for artifact in artifacts] != sorted(REQUIRED_ARTIFACTS):
        fail(f"trial receipt {path.name} has an incomplete artifact inventory")
    if any(artifact["size_bytes"] == 0 for artifact in artifacts):
        fail(f"trial receipt {path.name} contains an empty release artifact")
    release_artifact = next(
        artifact for artifact in artifacts if artifact["name"] == "release_manifest"
    )
    if release_artifact["sha256"] != "sha256:" + expected_release_manifest_sha256:
        fail(f"trial receipt {path.name} release artifact disagrees with its manifest")
    commands = validate_rows(
        value["commands"],
        "commands",
        {
            "argv_sha256",
            "duration_ms",
            "exit_code",
            "name",
            "stderr_sha256",
            "stdout_sha256",
        },
    )
    if [command["name"] for command in commands] != sorted(expected_checkpoints):
        fail(f"trial receipt {path.name} has an incomplete command inventory")
    if any(command["exit_code"] != 0 for command in commands):
        fail(f"trial receipt {path.name} contains a failed command")
    if sum(command["duration_ms"] for command in commands) > elapsed * 1000:
        fail(f"trial receipt {path.name} command durations exceed elapsed time")
    validate_rows(value["diagnostics"], "diagnostics", {"code", "resolution"})
    return Trial(
        trial_id=trial_id,
        participant_id=participant_id,
        journey=journey,
        route=route,
        profile=profile,
        elapsed_seconds=elapsed,
        receipt_sha256=hashlib.sha256(content).hexdigest(),
    )


def qualify(receipt_dir: Path, release_manifest: Path) -> dict[str, Any]:
    release_bytes = regular_bytes(release_manifest, "release manifest")
    release_value = strict_json(release_bytes, "release manifest")
    if release_value.get("schema") != "worldstream/release-artifact-manifest/v2":
        fail("release manifest has the wrong schema")
    release_sha256 = hashlib.sha256(release_bytes).hexdigest()
    if receipt_dir.is_symlink() or not receipt_dir.is_dir():
        fail("receipt directory must be a real directory")
    paths = sorted(receipt_dir.glob("*.json"), key=lambda path: path.name)
    if len(paths) > MAX_RECEIPTS:
        fail("receipt inventory exceeds its bound")
    trials = [validate_receipt(path, release_sha256) for path in paths]
    if len({trial.trial_id for trial in trials}) != len(trials):
        fail("trial IDs are not unique")
    if len({trial.participant_id for trial in trials}) != len(trials):
        fail("each qualification trial requires a distinct outside participant")
    pack = [trial for trial in trials if trial.journey == "pack_author"]
    integrators = [
        trial for trial in trials if trial.journey == "application_integrator"
    ]
    if len(pack) != 3 or len(integrators) != 3:
        fail("qualification requires exactly three Pack Authors and three integrators")
    if {trial.route for trial in pack} != PACK_ROUTES:
        fail("Pack Author trials must cover deterministic and prompt-assisted routes")
    profiles = sorted({trial.profile for trial in trials})
    if len(profiles) < 2:
        fail("qualification requires at least two installation profiles")
    return {
        "schema": SUMMARY_SCHEMA,
        "status": "qualified",
        "release_manifest_sha256": release_sha256,
        "trial_count": len(trials),
        "pack_author_count": len(pack),
        "application_integrator_count": len(integrators),
        "installation_profiles": profiles,
        "maximum_pack_author_seconds": max(trial.elapsed_seconds for trial in pack),
        "maximum_application_integrator_seconds": max(
            trial.elapsed_seconds for trial in integrators
        ),
        "receipts": [
            {
                "trial_id": trial.trial_id,
                "journey": trial.journey,
                "route": trial.route,
                "installation_profile": trial.profile,
                "receipt_sha256": trial.receipt_sha256,
            }
            for trial in sorted(trials, key=lambda trial: trial.trial_id)
        ],
    }


def object_value(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    return value


def bounded_code(value: object, label: str) -> str:
    if not isinstance(value, str) or SAFE_CODE.fullmatch(value) is None:
        fail(f"{label} must be a bounded safe code")
    return value


def utc_second(value: object, label: str) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{label} must be canonical UTC-second text")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise TrialError(f"{label} must be canonical UTC-second text") from error
    return parsed


def string_set(value: object, label: str) -> set[str]:
    if not isinstance(value, list) or len(value) > MAX_ROWS:
        fail(f"{label} must be a bounded list")
    result = {bounded_code(item, label) for item in value}
    if len(result) != len(value):
        fail(f"{label} contains duplicates")
    if value != sorted(value):
        fail(f"{label} must use deterministic sorted order")
    return result


def validate_rows(value: object, label: str, fields: set[str]) -> list[dict[str, Any]]:
    if not isinstance(value, list) or len(value) > MAX_ROWS:
        fail(f"{label} must be a bounded list")
    result: list[dict[str, Any]] = []
    for row in value:
        item = object_value(row, label)
        if set(item) != fields:
            fail(f"{label} row has the wrong field inventory")
        for field, content in item.items():
            if field in {"exit_code", "duration_ms", "size_bytes"}:
                if (
                    not isinstance(content, int)
                    or isinstance(content, bool)
                    or content < 0
                ):
                    fail(f"{label}.{field} must be a nonnegative integer")
            elif field in {"name", "code"}:
                bounded_code(content, f"{label}.{field}")
            elif field.endswith("_sha256") or field == "sha256":
                if (
                    not isinstance(content, str)
                    or not content.startswith("sha256:")
                    or SHA256.fullmatch(content.removeprefix("sha256:")) is None
                ):
                    fail(f"{label}.{field} must be a lowercase SHA-256 reference")
            elif (
                not isinstance(content, str)
                or len(content.encode("utf-8")) > MAX_TEXT_BYTES
            ):
                fail(f"{label}.{field} exceeds its text bound")
            elif re.search(
                r"(?i)(bearer\s+|wsb1:|api[_-]?key|password|secret)", content
            ):
                fail(f"{label}.{field} contains secret-like text")
        result.append(item)
    return result


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="validate six non-contributor WorldStream adopter receipts"
    )
    result.add_argument("--receipt-dir", required=True, type=Path)
    result.add_argument("--release-manifest", required=True, type=Path)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        summary = qualify(args.receipt_dir, args.release_manifest)
    except TrialError as error:
        print(f"outside-adopter qualification failed: {error}", file=sys.stderr)
        return 1
    os.write(sys.stdout.fileno(), canonical_json(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
