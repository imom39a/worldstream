#!/usr/bin/env python3
"""Run the packaged WorldStream Negotiate production-seam acceptance lane.

The lane executes the exact release `.wspack` twice in fresh
`worldstreamctl pack prove` processes. Each proof uses the production Bundle
Verifier, WASI-free Component Host, Core registry, all nine golden Actions,
authorized views, declared rejection, and retained-revision lookup. It also
runs the independent restart oracle, public TypeScript Pack suite, Console
reconnect/privacy suite, and offline dual-evidence verifier suite.

This receipt deliberately distinguishes a fresh Component Host process from a
live persisted-Room daemon restart. The latter remains a separate release
acceptance requirement and is never inferred from this lane.
"""

from __future__ import annotations

import argparse
import json
import os
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

import blake3

SCHEMA = "worldstream/negotiate-golden-flow-acceptance/v1"
ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BUNDLE = (
    ROOT
    / "packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack"
)
DEFAULT_CTL = ROOT / "target/debug/worldstreamctl"
CORPUS = ROOT / "crates/worldstream-negotiate-oracle/fixtures/corpus-v1.json"
OFFICIAL_EVIDENCE = ROOT / "packs/negotiate/evidence/conformance-v1.json"
MAX_JSON_BYTES = 32 * 1024 * 1024
MAX_BUNDLE_BYTES = 64 * 1024 * 1024
EXPECTED_ACTIONS = [
    "submit_proposal_revision",
    "submit_proposal_revision",
    "request_exact_approval",
    "record_exact_approval",
    "accept_current_proposal",
    "select_accepted_proposal",
    "record_agreement_signature",
    "record_agreement_signature",
    "commit_agreement",
]


class AcceptanceFailure(RuntimeError):
    """A bounded failure safe to report without command stderr."""


def strict_json(path: Path) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise AcceptanceFailure(
            f"required artifact is unavailable: {path.name}"
        ) from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise AcceptanceFailure(f"required artifact is not a regular file: {path.name}")
    if metadata.st_size <= 0 or metadata.st_size > MAX_JSON_BYTES:
        raise AcceptanceFailure(f"required JSON is outside its bound: {path.name}")

    def no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise AcceptanceFailure(f"duplicate JSON key in {path.name}")
            result[key] = value
        return result

    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=no_duplicates)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise AcceptanceFailure(f"required JSON is invalid: {path.name}") from error
    if not isinstance(value, dict):
        raise AcceptanceFailure(f"required JSON is not an object: {path.name}")
    return value


def validate_static_contract(bundle: Path) -> dict[str, Any]:
    corpus = strict_json(CORPUS)
    evidence = strict_json(OFFICIAL_EVIDENCE)
    golden = object_value(corpus.get("golden"), "golden corpus")
    steps = list_value(golden.get("steps"), "golden steps")
    actions = [
        object_value(object_value(step, "golden step").get("stimulus"), "stimulus")
        .get("value", {})
        .get("action")
        for step in steps
    ]
    if actions != EXPECTED_ACTIONS:
        raise AcceptanceFailure(
            "golden flow does not contain the frozen nine-Action order"
        )
    if golden.get("restart_after_step") != 4:
        raise AcceptanceFailure(
            "golden flow restart boundary is not after exact approval"
        )
    outcome = object_value(golden.get("outcome"), "golden Outcome")
    if outcome.get("kind") != "agreement_committed":
        raise AcceptanceFailure("golden flow did not establish agreement_committed")
    cross_index = list_value(golden.get("evidence_cross_index"), "evidence cross-index")
    if not cross_index:
        raise AcceptanceFailure("golden flow has no dual-evidence cross-index")
    bundle_evidence = object_value(evidence.get("bundle"), "official bundle evidence")
    expected_bundle = ROOT / str(bundle_evidence.get("path", ""))
    try:
        bundle_metadata = bundle.lstat()
    except OSError as error:
        raise AcceptanceFailure("selected release bundle is unavailable") from error
    if stat.S_ISLNK(bundle_metadata.st_mode) or not stat.S_ISREG(
        bundle_metadata.st_mode
    ):
        raise AcceptanceFailure("selected release bundle is not a regular file")
    if bundle_metadata.st_size <= 0 or bundle_metadata.st_size > MAX_BUNDLE_BYTES:
        raise AcceptanceFailure("selected release bundle is outside its byte bound")
    if (
        evidence.get("result") != "passed"
        or bundle.absolute() != expected_bundle.absolute()
    ):
        raise AcceptanceFailure(
            "official evidence does not bind the selected release bundle"
        )
    expected_digest = bundle_evidence.get("bundle_digest")
    if (
        not isinstance(expected_digest, str)
        or exact_blake3_digest(bundle) != expected_digest
    ):
        raise AcceptanceFailure(
            "selected release bundle bytes do not match official evidence"
        )
    return {
        "actions": actions,
        "restart_after_step": 4,
        "outcome": outcome["kind"],
        "evidence_cross_index_entries": len(cross_index),
        "bundle_digest": bundle_evidence.get("bundle_digest"),
        "revision_digest": bundle_evidence.get("revision_digest"),
        "component_digest": bundle_evidence.get("executor_component_digest"),
    }


def run_json(command: list[str], *, cwd: Path = ROOT) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
            env={**os.environ, "NO_COLOR": "1"},
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AcceptanceFailure("production proof command was unavailable") from error
    if completed.returncode != 0:
        raise AcceptanceFailure("production proof command failed closed")
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise AcceptanceFailure(
            "production proof did not emit one JSON receipt"
        ) from error
    if not isinstance(value, dict):
        raise AcceptanceFailure("production proof receipt was not an object")
    return value


def run_gate(command: list[str], name: str) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=600,
            env={**os.environ, "NO_COLOR": "1"},
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AcceptanceFailure(f"{name} gate was unavailable") from error
    if completed.returncode != 0:
        raise AcceptanceFailure(f"{name} gate failed closed")
    return {"name": name, "status": "passed", "exit_code": 0}


def validate_proof(proof: dict[str, Any], static: dict[str, Any]) -> None:
    expected = {
        "proof_type": "complete",
        "status": "passed",
        "pack_id": "worldstream.negotiate",
        "bundle_digest": static["bundle_digest"],
        "revision_digest": static["revision_digest"],
        "accepted_action": True,
        "declared_rejection": True,
        "retained_old_revision": True,
        "private_views": 4,
        "roles": 4,
    }
    if any(proof.get(key) != value for key, value in expected.items()):
        raise AcceptanceFailure(
            "production proof receipt did not satisfy the frozen contract"
        )
    transcript = proof.get("transcript_digest")
    if not isinstance(transcript, str) or not is_blake3_digest(transcript):
        raise AcceptanceFailure("production proof omitted its Core transcript identity")


def exact_blake3_digest(path: Path) -> str:
    hasher = blake3.blake3()
    try:
        with path.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                hasher.update(chunk)
    except OSError as error:
        raise AcceptanceFailure(
            "selected release bundle bytes were unavailable"
        ) from error
    return f"blake3:{hasher.hexdigest()}"


def is_blake3_digest(value: str) -> bool:
    if not value.startswith("blake3:") or len(value) != 71:
        return False
    return all(character in "0123456789abcdef" for character in value[7:])


def run_acceptance(worldstreamctl: Path, bundle: Path) -> dict[str, Any]:
    static = validate_static_contract(bundle)
    command = [str(worldstreamctl), "pack", "prove", str(bundle), "--json"]
    first = run_json(command)
    second = run_json(command)
    validate_proof(first, static)
    validate_proof(second, static)
    if first != second:
        raise AcceptanceFailure(
            "fresh production Host processes disagreed on the exact proof"
        )
    gates = [
        run_gate(
            [
                "cargo",
                "test",
                "-p",
                "worldstream-negotiate-oracle",
                "--test",
                "conformance",
                "golden_path_survives_restart_and_reconnect_byte_for_byte",
            ],
            "independent_restart_oracle",
        ),
        run_gate(
            ["pnpm", "--filter", "@worldstream/official-negotiate", "test"],
            "public_typescript_pack",
        ),
        run_gate(
            ["pnpm", "--filter", "@worldstream/console", "test"],
            "participant_reconnect_and_privacy",
        ),
        run_gate(
            [
                "cargo",
                "test",
                "-p",
                "worldstream-negotiate-evidence",
                "--test",
                "offline",
            ],
            "dual_evidence_offline_verifier",
        ),
    ]
    return {
        "schema": SCHEMA,
        "status": "passed",
        "release_evidence": False,
        "scope": {
            "production_bundle_verifier": True,
            "production_component_host": True,
            "production_core_registry": True,
            "fresh_component_host_process_restart": True,
            "durable_live_room_process_restart": False,
            "independent_participant_network_cursors": False,
        },
        "flow": static,
        "production_proof": first,
        "fresh_process_proof_equal": True,
        "gates": gates,
        "remaining_live_acceptance": [
            "persist one real Room through a forced worldstreamd restart",
            "reconnect independent buyer, seller, approver, and venue Membership cursors",
            "export that Room's dual proof package and verify it offline",
        ],
    }


def object_value(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AcceptanceFailure(f"{label} is not an object")
    return value


def list_value(value: object, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise AcceptanceFailure(f"{label} is not an array")
    return value


def canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worldstreamctl", type=Path, default=DEFAULT_CTL)
    parser.add_argument("--bundle", type=Path, default=DEFAULT_BUNDLE)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    try:
        report = (
            {
                "schema": SCHEMA,
                "status": "static_contract_valid",
                "release_evidence": False,
                "flow": validate_static_contract(args.bundle),
            }
            if args.validate_only
            else run_acceptance(args.worldstreamctl, args.bundle)
        )
        encoded = canonical(report)
        if args.report is not None:
            args.report.write_bytes(encoded)
        sys.stdout.buffer.write(encoded)
        return 0
    except AcceptanceFailure as error:
        print(
            json.dumps({"schema": SCHEMA, "status": "failed", "error": str(error)}),
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
