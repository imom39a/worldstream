#!/usr/bin/env python3
"""Produce typed pre-sign Runtime + Activity Pack release attestations.

Every input is a bounded machine report or an exact artifact.  The adapter
does not run tests, scrape logs, infer success from exit codes, or accept a
checkout-only/static/prototype result as release evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
MAX_JSON_BYTES = 64 * 1024 * 1024
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
SHA256 = re.compile(r"sha256:[0-9a-f]{64}\Z")
BLAKE3 = re.compile(r"blake3:[0-9a-f]{64}\Z")


class RuntimePackEvidenceError(RuntimeError):
    """A Runtime + Activity Pack diagnostic cannot support a release claim."""


def fail(message: str) -> None:
    raise RuntimePackEvidenceError(message)


def load_module(name: str, path: Path):
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


ADAPTER = load_module("worldstream_runtime_pack_release_adapter", ADAPTER_PATH)
STARTER = load_module(
    "worldstream_runtime_pack_bundle_identity", ROOT / "scripts/starter-distribution.py"
)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def regular_bytes(path: Path, label: str, maximum: int) -> bytes:
    try:
        admitted = path.lstat()
    except OSError as error:
        raise RuntimePackEvidenceError(f"{label} is unavailable") from error
    if (
        stat.S_ISLNK(admitted.st_mode)
        or not stat.S_ISREG(admitted.st_mode)
        or not (0 < admitted.st_size <= maximum)
    ):
        fail(f"{label} must be a bounded regular non-symlink file")
    try:
        content = path.read_bytes()
        finished = path.lstat()
    except OSError as error:
        raise RuntimePackEvidenceError(f"{label} could not be read") from error
    if len(content) != admitted.st_size or (
        admitted.st_dev,
        admitted.st_ino,
        admitted.st_size,
        admitted.st_mtime_ns,
    ) != (finished.st_dev, finished.st_ino, finished.st_size, finished.st_mtime_ns):
        fail(f"{label} changed while it was read")
    return content


def strict_json(path: Path, label: str) -> tuple[dict[str, Any], bytes]:
    content = regular_bytes(path, label, MAX_JSON_BYTES)
    try:
        value = ADAPTER.COLLECTOR.strict_json_object(content, label)
    except ADAPTER.COLLECTOR.CollectionError as error:
        raise RuntimePackEvidenceError(str(error)) from error
    return value, content


def artifact_binding(path: Path, label: str) -> dict[str, Any]:
    content = regular_bytes(path, label, MAX_ARTIFACT_BYTES)
    return {
        "sha256": "sha256:" + hashlib.sha256(content).hexdigest(),
        "size_bytes": len(content),
    }


def object_value(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    return value


def validate_release_contract(toml_path: Path, json_path: Path) -> dict[str, Any]:
    manifest = ADAPTER.COLLECTOR.load_manifest(toml_path, json_path)
    if (
        manifest.get("manifest_kind") != "release"
        or manifest.get("release_ready") is not True
        or manifest.get("unresolved_required_fields") != []
    ):
        fail(
            "Runtime + Pack release producers require a release-ready compatibility contract"
        )
    return manifest


def validate_pack_contract(
    bundle_path: Path, manifest: dict[str, Any]
) -> dict[str, str]:
    """Validate only the portable Bundle contract without making a release claim."""

    bundle = regular_bytes(bundle_path, "official Negotiate bundle", MAX_ARTIFACT_BYTES)
    try:
        identity = STARTER.pack_identity(bundle, "official Negotiate bundle")
        entries = STARTER.pack_entries(bundle, "official Negotiate bundle")
        conformance = entries["conformance.json"]
        dependency_lock = entries["dependency-lock.json"]
    except (STARTER.StarterError, KeyError) as error:
        raise RuntimePackEvidenceError(str(error)) from error
    try:
        evidence = ADAPTER.COLLECTOR.strict_json_object(
            conformance, "embedded official Negotiate conformance evidence"
        )
        dependencies = ADAPTER.COLLECTOR.strict_json_object(
            dependency_lock, "embedded official Negotiate dependency lock"
        )
    except ADAPTER.COLLECTOR.CollectionError as error:
        raise RuntimePackEvidenceError(str(error)) from error
    rows = [
        row
        for row in manifest.get("activity_pack_bundles", [])
        if isinstance(row, dict)
        and row.get("pack_id") == "worldstream.negotiate"
        and row.get("required_for_release") is True
    ]
    if len(rows) != 1:
        fail("compatibility contract has no unique official Negotiate bundle")
    row = rows[0]
    executor_rows = [
        item
        for item in manifest.get("pack_executors", [])
        if isinstance(item, dict)
        and item.get("pack_id") == "worldstream.negotiate"
        and item.get("required_for_release") is True
    ]
    if len(executor_rows) != 1:
        fail("compatibility contract has no unique Negotiate Pack executor")
    executor = executor_rows[0]
    toolchain = object_value(
        dependencies.get("toolchain"), "Negotiate dependency-lock toolchain"
    )
    if (
        identity.get("bundle_digest") != row.get("bundle_digest")
        or identity.get("revision_digest") != row.get("revision_digest")
        or evidence
        != {
            "conformance_id": "worldstream/pack-conformance/v1",
            "execution_profile_id": "worldstream/component-deterministic/v1",
            "executor_component_digest": executor.get("executor_artifact_digest"),
            "golden_corpus_digest": executor.get("golden_corpus_digest"),
            "passed": True,
            "revision_digest": identity.get("revision_digest"),
        }
        or toolchain
        != {
            "execution_profile_id": "worldstream/component-deterministic/v1",
            "host_contract": "worldstream/activity-pack/v1",
        }
    ):
        fail(
            "official Bundle embedded conformance and compatibility identities disagree"
        )
    return identity


def validate_pack(
    bundle_path: Path, proof_path: Path, manifest: dict[str, Any]
) -> tuple[dict[str, str], dict[str, str], dict[str, Any]]:
    identity = validate_pack_contract(bundle_path, manifest)
    proof, proof_bytes = strict_json(proof_path, "released worldstreamctl Pack proof")
    if (
        set(proof)
        != {
            "proof_type",
            "status",
            "pack_id",
            "bundle_digest",
            "revision_digest",
            "transcript_digest",
            "accepted_action",
            "declared_rejection",
            "private_views",
            "retained_old_revision",
            "room_id",
            "roles",
        }
        or proof.get("status") != "passed"
        or proof.get("proof_type") != "complete"
        or proof.get("pack_id") != identity.get("pack_id")
        or proof.get("bundle_digest") != identity.get("bundle_digest")
        or proof.get("revision_digest") != identity.get("revision_digest")
        or BLAKE3.fullmatch(str(proof.get("transcript_digest"))) is None
        or proof.get("accepted_action") is not True
        or proof.get("declared_rejection") is not True
        or proof.get("private_views") != 4
        or proof.get("retained_old_revision") is not True
        or not isinstance(proof.get("room_id"), str)
        or not proof["room_id"]
        or proof.get("roles") != 4
    ):
        fail(
            "released worldstreamctl Component Host/Core proof differs from the exact Bundle"
        )
    return (
        identity,
        {
            "bundle_exact_identity": "physical bundle, embedded conformance, semantic revision, and compatibility rows agree",
            "component_host_contract": "released worldstreamctl admitted the Component under the deterministic production Host contract",
            "production_core_proof": "released worldstreamctl Bundle verifier, Component Host, Core, rejection, and retained lookup passed",
            "resource_and_capability_denial": "the production Host admitted the WASI-free Component under its bounded capability profile",
        },
        {
            "sha256": "sha256:" + hashlib.sha256(proof_bytes).hexdigest(),
            "size_bytes": len(proof_bytes),
        },
    )


def validate_policy(path: Path, identity: dict[str, str]) -> dict[str, str]:
    report, _raw = strict_json(path, "Negotiate released-artifact policy qualification")
    artifact = object_value(report.get("artifact_identity"), "policy artifact identity")
    policy = object_value(report.get("source_policy"), "policy source boundary")
    scope = object_value(report.get("scope"), "Negotiate policy scope")
    flow = object_value(report.get("flow"), "Negotiate policy facts")
    proof = object_value(report.get("production_proof"), "Negotiate production proof")
    gates = report.get("gates")
    observed_gates = (
        {
            row.get("name"): row
            for row in gates
            if isinstance(gates, list) and isinstance(row, dict)
        }
        if isinstance(gates, list)
        else {}
    )
    required_gates = {
        "independent_restart_oracle",
        "public_typescript_pack",
        "participant_reconnect_and_privacy",
        "dual_evidence_offline_verifier",
    }
    if (
        set(report)
        != {
            "schema",
            "status",
            "release_evidence",
            "artifact_identity",
            "source_policy",
            "scope",
            "flow",
            "production_proof",
            "gates",
            "fresh_process_proof_equal",
            "independent_participant_network_cursors",
        }
        or report.get("schema") != "worldstream/negotiate-policy-qualification/v1"
        or report.get("status") != "passed"
        or report.get("release_evidence") is not True
        or artifact
        != {
            "bundle_digest": identity["bundle_digest"],
            "revision_digest": identity["revision_digest"],
        }
        or policy
        != {
            "released_artifacts_only": True,
            "source_checkout_access": False,
            "target_debug_access": False,
            "manual_database_access": False,
        }
        or report.get("fresh_process_proof_equal") is not True
        or report.get("independent_participant_network_cursors") is not True
        or flow.get("bundle_digest") != identity["bundle_digest"]
        or flow.get("revision_digest") != identity["revision_digest"]
        or flow.get("actions")
        != [
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
        or flow.get("outcome") != "agreement_committed"
        or proof.get("status") != "passed"
        or scope.get("production_bundle_verifier") is not True
        or scope.get("production_component_host") is not True
        or scope.get("production_core_registry") is not True
        or set(observed_gates) != required_gates
        or any(
            row.get("status") != "passed" or row.get("exit_code") != 0
            for row in observed_gates.values()
        )
    ):
        fail("Negotiate released-artifact oracle/privacy qualification is incomplete")
    return {
        "independent_oracle": "nine-Action committed flow and restart oracle passed",
        "participant_privacy": "six-persona privacy and reconnect corpus passed",
        "dual_evidence_verification": "independent offline party/venue evidence verification passed",
    }


def validate_a202(path: Path, artifact_binding: dict[str, Any]) -> str:
    report, _raw = strict_json(path, "A202 released-artifact qualification")
    profile = object_value(report.get("profile"), "A202 profile")
    checks = object_value(report.get("checks"), "A202 checks")
    artifact = object_value(report.get("artifact_identity"), "A202 artifact identity")
    policy = object_value(report.get("source_policy"), "A202 source boundary")
    expected_profile = {
        "repository_revision": "fa85aa8b49bfe7b3f7ded487c98500a600e92e41",
        "shared_objects": "a202-commercial/0.1",
        "state_machine_rules": "1.3",
        "operated_scope": "a202-scope/operated/0.1",
        "bilateral_scope": "a202-scope/bilateral/0.1",
        "demonstration_profile": "a202-profile/calibration-service/0.1",
    }
    required_checks = {
        "exact_canonical_objects",
        "signature_and_mandate_verification",
        "authenticated_resolver_evidence",
        "acceptance_selection_and_independent_signatures",
        "agreement_committed_cross_index",
        "negative_mutation_matrix",
    }
    if (
        set(report)
        != {
            "schema",
            "status",
            "release_evidence",
            "profile",
            "checks",
            "artifact_identity",
            "source_policy",
        }
        or report.get("schema") != "worldstream/a202-adapter-qualification/v1"
        or report.get("status") != "passed"
        or report.get("release_evidence") is not True
        or artifact != artifact_binding
        or policy
        != {
            "released_artifacts_only": True,
            "source_checkout_access": False,
            "target_debug_access": False,
        }
        or profile != expected_profile
        or set(checks) != required_checks
        or any(value is not True for value in checks.values())
    ):
        fail(
            "A202 released-artifact qualification does not prove the exact pinned operated profile"
        )
    return (
        "exact pinned A202 operated single-session profile and mutation matrix passed"
    )


def validate_restart(
    path: Path,
    profile: str,
    identity: dict[str, str],
    runtime_binding: dict[str, Any],
) -> dict[str, str]:
    report, _raw = strict_json(path, f"{profile} Negotiate restart diagnostic")
    artifact = object_value(
        report.get("artifact_identity"), "restart artifact identity"
    )
    room = object_value(report.get("room"), "restart Room facts")
    policy = object_value(report.get("policy"), "restart source policy")
    if (
        set(report)
        != {
            "schema",
            "status",
            "release_evidence",
            "storage_profile",
            "artifact_identity",
            "room",
            "policy",
        }
        or report.get("schema")
        != "worldstream/negotiate-released-artifact-acceptance/v1"
        or report.get("status") != "passed"
        or report.get("release_evidence") is not True
        or report.get("storage_profile") != profile
        or artifact
        != {
            "runtime_sha256": runtime_binding["sha256"],
            "runtime_size_bytes": runtime_binding["size_bytes"],
            "bundle_digest": identity["bundle_digest"],
            "revision_digest": identity["revision_digest"],
        }
        or room.get("pack_id") != "worldstream.negotiate"
        or room.get("forced_daemon_restart") is not True
        or room.get("independent_membership_cursors") != 4
        or room.get("all_memberships_reconnected") is not True
        or room.get("outcome") != "agreement_committed"
        or room.get("exact_executable_replay") is not True
        or room.get("head_before_restart") != room.get("replayed_head")
        or room.get("offline_dual_evidence") is not True
        or policy
        != {
            "released_artifacts_only": True,
            "source_checkout_access": False,
            "target_debug_access": False,
            "manual_database_access": False,
        }
    ):
        fail(f"{profile} Negotiate restart/Replay diagnostic is incomplete")
    return {
        "released_artifact_only": "exact detached runtime and Negotiate subjects were used without checkout binaries",
        "forced_restart_and_reconnect": "daemon restart and four independent Membership cursor reconnects passed",
        "exact_executable_replay": "retained exact executor reproduced the pre-restart canonical Head",
        "offline_dual_evidence": "exported party/venue proof package verified offline",
    }


def atomic_write(path: Path, value: object) -> None:
    content = canonical_json(value)
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink() or path.is_dir():
        fail(f"unsafe producer output: {path}")
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


def producer(
    manifest: dict[str, Any],
    source_id: str,
    outcomes: dict[str, str],
    artifacts: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    spec = ADAPTER.SOURCE_BY_ID[source_id]
    if set(outcomes) != set(spec.checks):
        fail(f"Runtime + Pack outcome mapping drifted: {source_id}")
    if set(artifacts) != set(ADAPTER.REQUIRED_ARTIFACT_BINDINGS[source_id]):
        fail(f"Runtime + Pack artifact mapping drifted: {source_id}")
    return {
        "schema": ADAPTER.PRODUCER_SCHEMA,
        "producer_id": ADAPTER.COLLECTOR.EXPECTED_PRODUCER_IDS[source_id],
        "evidence_id": spec.evidence_id,
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": ADAPTER.COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "passed",
                "observations": [{"kind": "verified-diagnostic", "value": value}],
            }
            for check, value in outcomes.items()
        },
        "artifacts": artifacts,
    }


def required_path(value: Path | None, label: str) -> Path:
    if value is None:
        fail(f"{label} is required for the selected Runtime + Pack producer")
    return value


def produce(args: argparse.Namespace) -> None:
    manifest = validate_release_contract(args.manifest_toml, args.manifest_json)
    bundle = required_path(args.bundle, "--bundle")
    pack_proof = required_path(args.pack_proof, "--pack-proof")
    linux_archive = required_path(args.linux_archive, "--linux-archive")
    runtime = artifact_binding(linux_archive, "native Linux release archive")
    identity, pack_outcomes, conformance_binding = validate_pack(
        bundle, pack_proof, manifest
    )
    bindings = {
        "bundle": artifact_binding(bundle, "official Negotiate bundle"),
        "pack_evidence": conformance_binding,
    }
    rows = {
        "pack-component-conformance": producer(
            manifest,
            "pack-component-conformance",
            pack_outcomes,
            {
                "negotiate-bundle": bindings["bundle"],
                "negotiate-conformance-evidence": bindings["pack_evidence"],
                "linux-release-profile": runtime,
            },
        ),
    }
    if not args.component_only:
        policy_report = required_path(args.policy_report, "--policy-report")
        a202_report = required_path(args.a202_report, "--a202-report")
        a202_artifact = required_path(args.a202_artifact, "--a202-artifact")
        sqlite_restart = required_path(
            args.sqlite_restart_report, "--sqlite-restart-report"
        )
        postgres_restart = required_path(
            args.postgres_restart_report, "--postgres-restart-report"
        )
        a202_binding = artifact_binding(a202_artifact, "A202 adapter release subject")
        policy_outcomes = validate_policy(policy_report, identity)
        policy_outcomes["a202_operated_profile"] = validate_a202(
            a202_report, a202_binding
        )
        sqlite_outcomes = validate_restart(
            sqlite_restart, "sqlite-bundled", identity, runtime
        )
        postgres_outcomes = validate_restart(
            postgres_restart, "postgres-primary", identity, runtime
        )
        bindings.update(
            {
                "policy": artifact_binding(
                    policy_report, "Negotiate released-artifact policy qualification"
                ),
                "a202_report": artifact_binding(
                    a202_report, "A202 released-artifact qualification"
                ),
                "a202_artifact": a202_binding,
                "sqlite": artifact_binding(sqlite_restart, "SQLite restart diagnostic"),
                "postgres": artifact_binding(
                    postgres_restart, "PostgreSQL restart diagnostic"
                ),
            }
        )
        rows["negotiate-policy"] = producer(
            manifest,
            "negotiate-policy",
            policy_outcomes,
            {
                "negotiate-policy-qualification": bindings["policy"],
                "a202-adapter-report": bindings["a202_report"],
                "a202-adapter": bindings["a202_artifact"],
            },
        )
        rows["negotiate-sqlite-restart"] = producer(
            manifest,
            "negotiate-sqlite-restart",
            sqlite_outcomes,
            {
                "sqlite-restart-replay-report": bindings["sqlite"],
                "linux-release-profile": runtime,
                "negotiate-bundle": bindings["bundle"],
            },
        )
        rows["negotiate-postgres-restart"] = producer(
            manifest,
            "negotiate-postgres-restart",
            postgres_outcomes,
            {
                "postgres-restart-replay-report": bindings["postgres"],
                "linux-release-profile": runtime,
                "negotiate-bundle": bindings["bundle"],
            },
        )
    if args.output_dir.exists() and args.output_dir.is_symlink():
        fail("producer output directory must not be a symlink")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    expected = {f"{source_id}.json" for source_id in rows}
    observed = {path.name for path in args.output_dir.iterdir()}
    if observed - expected:
        fail(
            f"producer output directory contains unexpected entries: {sorted(observed - expected)}"
        )
    for source_id, value in rows.items():
        atomic_write(args.output_dir / f"{source_id}.json", value)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument("--bundle", type=Path)
    command.add_argument("--pack-proof", type=Path)
    command.add_argument("--policy-report", type=Path)
    command.add_argument("--a202-report", type=Path)
    command.add_argument("--a202-artifact", type=Path)
    command.add_argument("--sqlite-restart-report", type=Path)
    command.add_argument("--postgres-restart-report", type=Path)
    command.add_argument("--linux-archive", type=Path)
    command.add_argument(
        "--component-only",
        action="store_true",
        help="emit only the bundle/Component Host/Core conformance producer",
    )
    command.add_argument(
        "--manifest-toml", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        produce(args)
    except (
        RuntimePackEvidenceError,
        KeyError,
        StopIteration,
        TypeError,
        ValueError,
    ) as error:
        print(f"Runtime + Pack evidence production failed: {error}", file=sys.stderr)
        return 1
    count = 1 if args.component_only else 4
    print(f"produced {count} exact-artifact Runtime + Pack release producers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
