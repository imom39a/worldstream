#!/usr/bin/env python3
"""Adapt typed producer results into collector source reports.

This boundary accepts producer attestations only in the strict
``worldstream/release-evidence-producer/v1`` shape.  It computes collector
check results from typed outcome statuses and verifies every declared artifact
binding from bytes; it never accepts a checks/boolean map, a filename, or an
exit code as evidence.  Invalid or unavailable producers become explicit
fail-closed source reports, which the collector will reject rather than
promote.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
COLLECTOR_PATH = ROOT / "scripts/release-evidence-collect.py"
PRODUCER_SCHEMA = "worldstream/release-evidence-producer/v1"
PRODUCER_FIELDS = {
    "schema",
    "producer_id",
    "evidence_id",
    "status",
    "release_evidence",
    "fail_closed",
    "phase",
    "version",
    "platform",
    "contract",
    "outcomes",
    "artifacts",
}
OUTCOME_FIELDS = {"status", "observations"}
ARTIFACT_FIELDS = {"sha256", "size_bytes"}
HASH_PREFIX = "sha256:"
STATUS_VALUES = frozenset({"passed", "failed", "unavailable", "incomplete"})


def load_collector():
    spec = importlib.util.spec_from_file_location(
        "release_evidence_collect", COLLECTOR_PATH
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load collector: {COLLECTOR_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


COLLECTOR = load_collector()
SOURCE_SPECS = COLLECTOR.SOURCE_SPECS
SOURCE_BY_ID = COLLECTOR.SOURCE_BY_ID
REQUIRED_ARTIFACT_BINDINGS = COLLECTOR.REQUIRED_ARTIFACT_BINDINGS


class ProducerError(RuntimeError):
    """A producer result cannot be promoted to a release source report."""


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise ProducerError(f"missing {label}: {path}") from error
    except OSError as error:
        raise ProducerError(f"cannot inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(mode):
        raise ProducerError(f"{label} must not be a symlink: {path}")
    if not stat.S_ISREG(mode):
        raise ProducerError(f"{label} must be a regular file: {path}")
    return path


def parse_assignments(
    values: list[str], expected: set[str], label: str
) -> dict[str, str]:
    parsed: dict[str, str] = {}
    for value in values:
        if "=" not in value:
            raise ProducerError(f"{label} must use ID=VALUE syntax: {value!r}")
        key, item = value.split("=", 1)
        if key not in expected:
            raise ProducerError(f"unknown {label} ID: {key}")
        if key in parsed:
            raise ProducerError(f"duplicate {label} ID: {key}")
        if not item:
            raise ProducerError(f"empty {label} value for {key}")
        parsed[key] = item
    return parsed


def parse_artifacts(
    values: list[str], expected: set[str]
) -> dict[tuple[str, str], Path]:
    parsed: dict[tuple[str, str], Path] = {}
    for value in values:
        if "=" not in value or "/" not in value.split("=", 1)[0]:
            raise ProducerError(
                f"artifact must use SOURCE_ID/BINDING_ID=PATH syntax: {value!r}"
            )
        binding, raw_path = value.split("=", 1)
        source_id, binding_id = binding.split("/", 1)
        if source_id not in expected:
            raise ProducerError(f"unknown artifact source ID: {source_id}")
        if not binding_id:
            raise ProducerError(f"empty artifact binding ID for {source_id}")
        key = (source_id, binding_id)
        if key in parsed:
            raise ProducerError(f"duplicate artifact binding: {source_id}/{binding_id}")
        parsed[key] = Path(raw_path)
    return parsed


def sha256_file(path: Path) -> tuple[str, int]:
    regular_file(path, "artifact binding")
    try:
        data = path.read_bytes()
    except OSError as error:
        raise ProducerError(f"cannot read artifact binding {path}: {error}") from error
    return HASH_PREFIX + hashlib.sha256(data).hexdigest(), len(data)


def checked_status(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ProducerError(f"{label} status must be a string")
    if value not in STATUS_VALUES:
        raise ProducerError(f"{label} status is invalid: {value!r}")
    return value


def read_producer(path: Path, spec, manifest: dict[str, Any]) -> dict[str, Any]:
    regular_file(path, f"producer result {spec.source_id}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProducerError(f"producer result is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ProducerError("producer result must be a JSON object")
    missing = sorted(PRODUCER_FIELDS - set(value))
    extra = sorted(set(value) - PRODUCER_FIELDS)
    if missing or extra:
        raise ProducerError(
            f"producer result has wrong fields: missing={missing}; extra={extra}"
        )
    if value["schema"] != PRODUCER_SCHEMA:
        raise ProducerError(
            f"producer result has wrong schema: expected={PRODUCER_SCHEMA!r}; "
            f"observed={value['schema']!r}"
        )
    expected_producer_id = COLLECTOR.EXPECTED_PRODUCER_IDS[spec.source_id]
    if value["producer_id"] != expected_producer_id:
        raise ProducerError(
            f"producer_id mismatch: expected={expected_producer_id!r}; "
            f"observed={value['producer_id']!r}"
        )
    if value["evidence_id"] != spec.evidence_id:
        raise ProducerError(
            f"producer evidence_id mismatch: expected={spec.evidence_id!r}; "
            f"observed={value['evidence_id']!r}"
        )
    checked_status(value["status"], "producer")
    if (
        type(value["release_evidence"]) is not bool
        or type(value["fail_closed"]) is not bool
    ):
        raise ProducerError(
            "producer release_evidence and fail_closed must be booleans"
        )
    if value["version"] != manifest["release_candidate"]:
        raise ProducerError(
            f"producer version mismatch: expected={manifest['release_candidate']!r}; "
            f"observed={value['version']!r}"
        )
    if value["platform"] != spec.platform:
        raise ProducerError(
            f"producer platform mismatch: expected={spec.platform!r}; "
            f"observed={value['platform']!r}"
        )
    if value["contract"] != manifest["contracts"]:
        raise ProducerError("producer compatibility contract mismatch")
    if value["phase"] != COLLECTOR.PRE_SIGN_PHASE:
        raise ProducerError(
            f"producer phase must be {COLLECTOR.PRE_SIGN_PHASE!r}; "
            f"observed={value['phase']!r}"
        )
    outcomes = value["outcomes"]
    if not isinstance(outcomes, dict) or set(outcomes) != set(spec.checks):
        raise ProducerError(
            f"producer outcomes must exactly match checks: expected={sorted(spec.checks)}"
        )
    for check in spec.checks:
        outcome = outcomes[check]
        if not isinstance(outcome, dict) or set(outcome) != OUTCOME_FIELDS:
            raise ProducerError(f"producer outcome {check} has wrong fields")
        checked_status(outcome["status"], f"producer outcome {check}")
        observations = outcome["observations"]
        if not isinstance(observations, list) or not observations:
            raise ProducerError(f"producer outcome {check} has no observations")
        for observation in observations:
            if (
                not isinstance(observation, dict)
                or set(observation) != {"kind", "value"}
                or not isinstance(observation["kind"], str)
                or not observation["kind"]
                or not isinstance(observation["value"], str)
                or not observation["value"]
            ):
                raise ProducerError(
                    f"producer outcome {check} has invalid observations"
                )
    artifacts = value["artifacts"]
    if not isinstance(artifacts, dict):
        raise ProducerError("producer artifacts must be an object")
    required_bindings = set(REQUIRED_ARTIFACT_BINDINGS[spec.source_id])
    if set(artifacts) != required_bindings:
        raise ProducerError(
            f"producer artifact bindings must exactly be {sorted(required_bindings)}"
        )
    for binding_id, artifact in artifacts.items():
        if not isinstance(binding_id, str) or not binding_id:
            raise ProducerError("producer artifact binding ID must be a string")
        if not isinstance(artifact, dict) or set(artifact) != ARTIFACT_FIELDS:
            raise ProducerError(f"producer artifact {binding_id} has wrong fields")
        digest = artifact["sha256"]
        if (
            not isinstance(digest, str)
            or len(digest) != len(HASH_PREFIX) + 64
            or not digest.startswith(HASH_PREFIX)
            or any(
                character not in "0123456789abcdef"
                for character in digest[len(HASH_PREFIX) :]
            )
        ):
            raise ProducerError(f"producer artifact {binding_id} has invalid SHA-256")
        if type(artifact["size_bytes"]) is not int or artifact["size_bytes"] < 0:
            raise ProducerError(
                f"producer artifact {binding_id} has invalid size_bytes"
            )
    return value


def verify_artifacts(
    source_id: str,
    producer: dict[str, Any],
    artifact_paths: dict[tuple[str, str], Path],
) -> None:
    declared = set(producer["artifacts"])
    supplied = {
        binding_id
        for current_source, binding_id in artifact_paths
        if current_source == source_id
    }
    if declared != supplied:
        raise ProducerError(
            f"artifact bindings differ for {source_id}: "
            f"missing={sorted(declared - supplied)}; extra={sorted(supplied - declared)}"
        )
    for binding_id, expected in producer["artifacts"].items():
        observed_digest, observed_size = sha256_file(
            artifact_paths[(source_id, binding_id)]
        )
        if observed_digest != expected["sha256"]:
            raise ProducerError(
                f"artifact {binding_id} digest mismatch: expected={expected['sha256']}; "
                f"observed={observed_digest}"
            )
        if observed_size != expected["size_bytes"]:
            raise ProducerError(
                f"artifact {binding_id} size mismatch: expected={expected['size_bytes']}; "
                f"observed={observed_size}"
            )


def source_report(
    spec, manifest: dict[str, Any], *, producer=None, reason=None
) -> dict[str, Any]:
    if producer is not None and reason is None:
        passed = (
            producer["status"] == "passed"
            and producer["release_evidence"] is True
            and producer["fail_closed"] is False
            and all(
                producer["outcomes"][check]["status"] == "passed"
                for check in spec.checks
            )
        )
        if not passed:
            reason = "producer outcome is not an explicit passed release attestation"
    if reason is None:
        reason = "producer result validated"
    passed = producer is not None and reason == "producer result validated"
    details: dict[str, Any] = {
        "producer_id": producer["producer_id"] if producer else None,
        "phase": producer["phase"] if producer else COLLECTOR.PRE_SIGN_PHASE,
        "reason": reason,
        "missing_producer": (
            None
            if passed
            else (
                reason.removeprefix("missing producer:").strip()
                if reason.startswith("missing producer:")
                else f"{spec.source_id}-release-evidence-producer"
            )
        ),
        "outcomes": producer["outcomes"] if producer else {},
        "artifacts": producer["artifacts"] if producer else {},
    }
    if passed:
        details.pop("reason")
        details.pop("missing_producer")
    return {
        "schema": spec.schema,
        "source_id": spec.source_id,
        "evidence_id": spec.evidence_id,
        "status": "passed" if passed else "unavailable",
        "release_evidence": passed,
        "fail_closed": not passed,
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "checks": {
            check: producer["outcomes"][check]["status"] == "passed"
            if producer
            else False
            for check in spec.checks
        },
        "details": details,
    }


def atomic_write(path: Path, content: bytes) -> None:
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def produce(
    output_dir: Path,
    producers: dict[str, Path],
    missing: dict[str, str],
    artifact_paths: dict[tuple[str, str], Path],
    manifest_toml: Path = COLLECTOR.DEFAULT_MANIFEST_TOML,
    manifest_json: Path = COLLECTOR.DEFAULT_MANIFEST_JSON,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    manifest = COLLECTOR.load_manifest(manifest_toml, manifest_json)
    evidence_ids = COLLECTOR.release_evidence_ids(manifest)
    specs = COLLECTOR.validate_mapping(evidence_ids)
    expected_sources = {spec.source_id for spec in specs}
    if set(producers) | set(missing) != expected_sources or set(producers) & set(
        missing
    ):
        raise ProducerError(
            "producer inputs must cover each source exactly once with --producer or --missing"
        )
    for source_id, _binding_id in artifact_paths:
        if source_id not in expected_sources:
            raise ProducerError(f"artifact binding names unknown source: {source_id}")
    supplied_artifact_sources = {source_id for source_id, _ in artifact_paths}
    unavailable_with_artifacts = supplied_artifact_sources & set(missing)
    if unavailable_with_artifacts:
        raise ProducerError(
            "artifact bindings cannot be supplied without producers: "
            + ", ".join(sorted(unavailable_with_artifacts))
        )
    if output_dir.exists() and output_dir.is_symlink():
        raise ProducerError(f"output directory must not be a symlink: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)
    expected_files = {evidence_id + ".json" for evidence_id in evidence_ids}
    for candidate in output_dir.rglob("*"):
        relative = candidate.relative_to(output_dir).as_posix()
        if (
            candidate.is_symlink()
            or candidate.is_dir()
            or relative not in expected_files
        ):
            raise ProducerError(
                f"output directory contains an unexpected entry: {relative}"
            )

    reports: dict[str, bytes] = {}
    failed: list[str] = []
    for spec in specs:
        if spec.source_id in missing:
            failed.append(spec.source_id)
            missing_reason = missing[spec.source_id]
            if not missing_reason.startswith("missing producer:"):
                missing_reason = f"missing producer: {missing_reason}"
            report = source_report(
                spec,
                manifest,
                reason=missing_reason,
            )
        else:
            try:
                producer = read_producer(producers[spec.source_id], spec, manifest)
                verify_artifacts(spec.source_id, producer, artifact_paths)
                report = source_report(spec, manifest, producer=producer)
                if report["release_evidence"] is not True:
                    failed.append(spec.source_id)
            except ProducerError as error:
                failed.append(spec.source_id)
                report = source_report(spec, manifest, reason=str(error))
        reports[spec.evidence_id] = canonical_json(report)

    for evidence_id in evidence_ids:
        atomic_write(output_dir / f"{evidence_id}.json", reports[evidence_id])
    return evidence_ids, tuple(sorted(failed))


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument(
        "--producer", action="append", default=[], metavar="SOURCE_ID=PATH"
    )
    command.add_argument(
        "--missing", action="append", default=[], metavar="SOURCE_ID=REASON"
    )
    command.add_argument(
        "--artifact",
        action="append",
        default=[],
        metavar="SOURCE_ID/BINDING_ID=PATH",
    )
    command.add_argument(
        "--manifest-toml", type=Path, default=COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        manifest = COLLECTOR.load_manifest(args.manifest_toml, args.manifest_json)
        specs = COLLECTOR.validate_mapping(COLLECTOR.release_evidence_ids(manifest))
        expected = {spec.source_id for spec in specs}
        producers = parse_assignments(args.producer, expected, "producer")
        missing = parse_assignments(args.missing, expected, "missing")
        artifacts = parse_artifacts(args.artifact, expected)
        producer_paths = {
            source_id: Path(path) for source_id, path in producers.items()
        }
        evidence_ids, failed = produce(
            args.output_dir,
            producer_paths,
            missing,
            artifacts,
            args.manifest_toml,
            args.manifest_json,
        )
    except ProducerError as error:
        print(f"release evidence production failed: {error}", file=sys.stderr)
        return 1
    print(
        f"produced {len(evidence_ids)} source reports; "
        f"fail-closed sources={len(failed)}: {', '.join(failed) if failed else 'none'}"
    )
    if failed:
        for source_id in failed:
            spec = SOURCE_BY_ID[source_id]
            report_path = args.output_dir / f"{spec.evidence_id}.json"
            report = json.loads(report_path.read_text(encoding="utf-8"))
            details = report.get("details", {})
            print(
                f"fail-closed producer: source={source_id}; "
                f"missing_producer={details.get('missing_producer')!r}; "
                f"reason={details.get('reason')!r}",
                file=sys.stderr,
            )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
