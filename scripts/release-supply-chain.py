#!/usr/bin/env python3
"""Produce and verify the non-circular release supply chain.

The unsigned aggregation phase copies four payloads and exactly thirteen
non-supply-chain typed source reports into a canonical inventory, then creates
and validates SHA256SUMS, SPDX, and SLSA over that closed set. A separate,
minimal OIDC job signs only that exact inventory. A later no-OIDC phase
identity-verifies the signature and emits the typed fourteenth producer report
before final deterministic assembly. A second minimal OIDC job signs that exact
manifest, and a final no-OIDC phase verifies both detached signature levels.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCRIPT = Path(__file__).with_name("release-evidence-assemble.py")
SPEC = importlib.util.spec_from_file_location("release_evidence_assemble", SCRIPT)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError(f"cannot load {SCRIPT}")
ASSEMBLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ASSEMBLER)

PRODUCER_SCRIPT = Path(__file__).with_name("release-evidence-produce.py")
PRODUCER_SPEC = importlib.util.spec_from_file_location(
    "release_evidence_produce", PRODUCER_SCRIPT
)
if PRODUCER_SPEC is None or PRODUCER_SPEC.loader is None:  # pragma: no cover
    raise RuntimeError(f"cannot load {PRODUCER_SCRIPT}")
PRODUCER = importlib.util.module_from_spec(PRODUCER_SPEC)
PRODUCER_SPEC.loader.exec_module(PRODUCER)

IDENTITY_SCRIPT = Path(__file__).with_name("release_build_identity.py")
IDENTITY_SPEC = importlib.util.spec_from_file_location(
    "release_build_identity", IDENTITY_SCRIPT
)
if IDENTITY_SPEC is None or IDENTITY_SPEC.loader is None:  # pragma: no cover
    raise RuntimeError(f"cannot load {IDENTITY_SCRIPT}")
IDENTITY = importlib.util.module_from_spec(IDENTITY_SPEC)
IDENTITY_SPEC.loader.exec_module(IDENTITY)

SUPPLY_CHAIN_SOURCE_ID = "supply-chain"
SUPPLY_CHAIN_EVIDENCE_ID = "checksums-signature-sbom-provenance"
INVENTORY_SCHEMA = "worldstream/release-subject-inventory/v1"
PRE_SIGN_PHASE = "pre-sign"
SUPPLY_CHAIN_CHECKS = (
    "checksums",
    "sigstore_identity",
    "spdx_subjects",
    "slsa_subjects",
)
SPDX_CREATOR = "Tool: worldstream-release-supply-chain-1.0"
ARTIFACT_PATHS = {
    "subject-inventory": ASSEMBLER.PRE_SIGN_INVENTORY_PATH,
    "subject-signature": ASSEMBLER.PRE_SIGN_SIGNATURE_PATH,
    "checksums": ASSEMBLER.SIDECAR_PATHS["checksums"],
    "spdx-sbom": ASSEMBLER.SIDECAR_PATHS["spdx-sbom"],
    "slsa-provenance": ASSEMBLER.SIDECAR_PATHS["slsa-provenance"],
}
OIDC_ENVIRONMENT_KEYS = (
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
)


def reject_oidc_environment(label: str) -> None:
    """Fail if a phase that must be unprivileged can request an OIDC token."""

    if any(os.environ.get(name) for name in OIDC_ENVIRONMENT_KEYS):
        ASSEMBLER.fail(f"{label} must run without an Actions OIDC capability")


def now_iso() -> str:
    source_date_epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if source_date_epoch is not None:
        try:
            timestamp = datetime.fromtimestamp(int(source_date_epoch), tz=timezone.utc)
            return timestamp.strftime("%Y-%m-%dT%H:%M:%SZ")
        except ValueError as error:
            ASSEMBLER.fail(f"SOURCE_DATE_EPOCH is not an integer: {error}")
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_sources(values: list[str], expected: set[str]) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    for value in values:
        if "=" not in value:
            ASSEMBLER.fail(f"source must use SOURCE_ID=PATH syntax: {value!r}")
        source_id, raw_path = value.split("=", 1)
        if source_id not in expected:
            ASSEMBLER.fail(f"unknown pre-sign source ID: {source_id}")
        if source_id in parsed:
            ASSEMBLER.fail(f"duplicate pre-sign source ID: {source_id}")
        path = Path(raw_path)
        ASSEMBLER.regular_file(path, f"pre-sign source report {source_id}")
        parsed[source_id] = path
    if set(parsed) != expected:
        ASSEMBLER.fail(
            "pre-sign source reports must explicitly cover every non-supply-chain source: "
            f"missing={sorted(expected - set(parsed))}; "
            f"extra={sorted(set(parsed) - expected)}"
        )
    return parsed


def source_specs() -> tuple[Any, ...]:
    return tuple(
        spec
        for spec in PRODUCER.COLLECTOR.SOURCE_SPECS
        if spec.source_id != SUPPLY_CHAIN_SOURCE_ID
    )


def copy_payloads(
    release_dir: Path, payload_dir: Path, version: str
) -> dict[str, Path]:
    payloads = ASSEMBLER.validate_payload_inputs(payload_dir, version)
    names = ASSEMBLER.payload_names(version)
    paths: dict[str, Path] = {}
    for artifact_id, source in payloads.items():
        destination = release_dir / names[artifact_id]
        ASSEMBLER.copy_atomic(
            source,
            destination,
            f"payload {artifact_id}",
            maximum=ASSEMBLER.MAX_RELEASE_PAYLOAD_BYTES,
        )
        paths[artifact_id] = destination
    return paths


def copy_source_reports(
    release_dir: Path, sources: dict[str, Path], manifest: dict[str, Any]
) -> dict[str, Path]:
    paths: dict[str, Path] = {}
    for spec in source_specs():
        loaded = PRODUCER.COLLECTOR.source_report(
            sources[spec.source_id], spec, manifest
        )
        destination = (
            release_dir
            / ASSEMBLER.PRE_SIGN_SUBJECT_DIRECTORY
            / f"{spec.evidence_id}.json"
        )
        ASSEMBLER.atomic_write_bytes(
            destination, loaded["raw"], f"pre-sign source report {spec.source_id}"
        )
        paths[spec.evidence_id] = destination
    return paths


def inventory_value(
    version: str,
    payload_paths: dict[str, Path],
    source_paths: dict[str, Path],
    release_dir: Path,
) -> dict[str, Any]:
    subjects: list[dict[str, Any]] = []
    for artifact_id, path in payload_paths.items():
        subjects.append(
            {
                "kind": "payload",
                "id": artifact_id,
                "path": path.relative_to(release_dir).as_posix(),
                "sha256": "sha256:" + ASSEMBLER.sha256_file(path),
                "size_bytes": path.stat().st_size,
            }
        )
    for evidence_id, path in source_paths.items():
        subjects.append(
            {
                "kind": "source-report",
                "id": evidence_id,
                "path": path.relative_to(release_dir).as_posix(),
                "sha256": "sha256:" + ASSEMBLER.sha256_file(path),
                "size_bytes": path.stat().st_size,
            }
        )
    subjects.sort(key=lambda item: (item["kind"], item["id"]))
    return {
        "schema": INVENTORY_SCHEMA,
        "phase": PRE_SIGN_PHASE,
        "product": version,
        "subjects": subjects,
    }


def spdx_document_namespace(
    version: str,
    mirror_digest: str,
    subjects: dict[str, Path],
    created: str,
) -> str:
    """Delegate the exact namespace contract to the shared identity verifier."""

    return IDENTITY.spdx_document_namespace(version, mirror_digest, subjects, created)


def generate_spdx(
    version: str,
    mirror_digest: str,
    subjects: dict[str, Path],
    created: str,
    payload_paths: dict[str, Path],
) -> dict[str, Any]:
    try:
        identities, source_entries = IDENTITY.release_payload_identities(
            payload_paths, version
        )
        revision = identities["source-archive"]["source"]["revision"]
        packages, relationships, document_describes = IDENTITY.spdx_graph(
            version=version,
            revision=revision,
            subjects=subjects,
            identities=identities,
            source_entries=source_entries,
        )
    except IDENTITY.IdentityError as error:
        ASSEMBLER.fail(f"release payload build identity rejected: {error}")
    files = []
    for relative, path in sorted(subjects.items()):
        files.append(
            {
                "SPDXID": IDENTITY._spdx_id("ReleaseSubject", relative),
                "fileName": relative,
                "checksums": [
                    {
                        "algorithm": "SHA1",
                        "checksumValue": ASSEMBLER.sha1_file(path),
                    },
                    {
                        "algorithm": "SHA256",
                        "checksumValue": ASSEMBLER.sha256_file(path),
                    },
                ],
                "copyrightText": "NOASSERTION",
                "licenseConcluded": "NOASSERTION",
            }
        )
    return {
        "spdxVersion": "SPDX-2.3",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"WorldStream {version} pre-sign release subjects",
        "dataLicense": "CC0-1.0",
        "documentNamespace": spdx_document_namespace(
            version, mirror_digest, subjects, created
        ),
        "creationInfo": {
            "created": created,
            "creators": [SPDX_CREATOR],
        },
        "packages": packages,
        "files": files,
        "relationships": relationships,
        "documentDescribes": document_describes,
    }


def generate_provenance(
    version: str,
    subjects: dict[str, Path],
    created: str,
    payload_paths: dict[str, Path],
) -> dict[str, Any]:
    try:
        identities, source_entries = IDENTITY.release_payload_identities(
            payload_paths, version
        )
        revision = identities["source-archive"]["source"]["revision"]
        invocation_parameters = IDENTITY.release_invocation_parameters()
        graph, aggregation_result = IDENTITY.provenance_graph(
            version=version,
            revision=revision,
            subjects=payload_paths,
            release_subjects=subjects,
            identities=identities,
            source_entries=source_entries,
            invocation_parameters=invocation_parameters,
        )
        run_details = IDENTITY.github_run_details(aggregation_result)
    except IDENTITY.IdentityError as error:
        ASSEMBLER.fail(f"release payload build identity rejected: {error}")
    return {
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [
            {
                "name": relative,
                "digest": {"sha256": ASSEMBLER.sha256_file(path)},
            }
            for relative, path in sorted(subjects.items())
        ],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": IDENTITY.BUILD_TYPE,
                **graph,
            },
            "runDetails": run_details,
        },
    }


def sign_and_verify(path: Path, bundle: Path) -> tuple[str, str]:
    cosign = shutil.which("cosign")
    identity = os.environ.get("COSIGN_CERTIFICATE_IDENTITY")
    issuer = os.environ.get("COSIGN_CERTIFICATE_OIDC_ISSUER")
    if not cosign or not identity or not issuer:
        ASSEMBLER.fail(
            "pre-sign subject signing requires cosign, COSIGN_CERTIFICATE_IDENTITY, "
            "and COSIGN_CERTIFICATE_OIDC_ISSUER"
        )
    result = subprocess.run(
        [cosign, "sign-blob", "--yes", "--bundle", str(bundle), str(path)],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not bundle.is_file() or bundle.is_symlink():
        ASSEMBLER.fail("cosign could not sign the pre-sign subject inventory")
    result = subprocess.run(
        [
            cosign,
            "verify-blob",
            "--bundle",
            str(bundle),
            "--certificate-identity",
            identity,
            "--certificate-oidc-issuer",
            issuer,
            str(path),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        ASSEMBLER.fail(
            "cosign could not identity-verify the pre-sign subject inventory"
        )
    return identity, issuer


def artifact_bindings(release_dir: Path) -> dict[str, dict[str, Any]]:
    return {
        binding: {
            "sha256": "sha256:" + ASSEMBLER.sha256_file(release_dir / relative),
            "size_bytes": (release_dir / relative).stat().st_size,
        }
        for binding, relative in ARTIFACT_PATHS.items()
    }


def typed_producer(
    manifest: dict[str, Any],
    inventory: dict[str, Any],
    release_dir: Path,
    identity: str,
    issuer: str,
) -> dict[str, Any]:
    observations = {
        "checksums": f"SHA256SUMS covers {len(inventory['subjects'])} signed subjects",
        "sigstore_identity": (
            f"identity={identity};issuer={issuer};bundle=identity-verified"
        ),
        "spdx_subjects": f"SPDX-2.3 covers {len(inventory['subjects'])} signed subjects",
        "slsa_subjects": f"SLSA v1 covers {len(inventory['subjects'])} signed subjects",
    }
    return {
        "schema": PRODUCER.PRODUCER_SCHEMA,
        "producer_id": "release-supply-chain-pre-sign-v1",
        "evidence_id": SUPPLY_CHAIN_EVIDENCE_ID,
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": PRODUCER.SOURCE_BY_ID[SUPPLY_CHAIN_SOURCE_ID].platform,
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "passed",
                "observations": [{"kind": "verified", "value": observations[check]}],
            }
            for check in SUPPLY_CHAIN_CHECKS
        },
        "artifacts": artifact_bindings(release_dir),
    }


def validate_prepared_material(
    release_dir: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Recompute the complete unsigned subject graph from assembled bytes."""

    manifest, _authored, mirror_bytes = ASSEMBLER.load_manifest(
        manifest_toml, manifest_json
    )
    version, evidence_ids = ASSEMBLER.validate_release_contract(manifest)
    ASSEMBLER.validate_existing_release_dir(release_dir, version, evidence_ids)
    payload_paths = {
        artifact_id: ASSEMBLER.regular_file(
            release_dir / relative, f"prepared payload {artifact_id}"
        )
        for artifact_id, relative in ASSEMBLER.payload_names(version).items()
    }
    source_paths = {
        spec.evidence_id: ASSEMBLER.regular_file(
            release_dir
            / ASSEMBLER.PRE_SIGN_SUBJECT_DIRECTORY
            / f"{spec.evidence_id}.json",
            f"prepared source report {spec.source_id}",
        )
        for spec in source_specs()
    }
    ASSEMBLER.validate_release_payloads(
        payload_paths, source_paths, ASSEMBLER.sha256_bytes(mirror_bytes)
    )
    expected_inventory = inventory_value(
        version, payload_paths, source_paths, release_dir
    )
    inventory_path = release_dir / ASSEMBLER.PRE_SIGN_INVENTORY_PATH
    inventory = ASSEMBLER.json_object(inventory_path, "pre-sign subject inventory")
    if inventory != expected_inventory:
        ASSEMBLER.fail(
            "pre-sign subject inventory differs from the exact assembled subjects"
        )
    subjects = {
        item["path"]: release_dir / item["path"] for item in inventory["subjects"]
    }
    checksums_path = release_dir / ASSEMBLER.SIDECAR_PATHS["checksums"]
    if ASSEMBLER.bounded_regular_bytes(
        checksums_path, "prepared SHA256SUMS"
    ) != ASSEMBLER.checksums_bytes(subjects):
        ASSEMBLER.fail("prepared SHA256SUMS differs from the exact subject inventory")
    spdx_path = release_dir / ASSEMBLER.SIDECAR_PATHS["spdx-sbom"]
    provenance_path = release_dir / ASSEMBLER.SIDECAR_PATHS["slsa-provenance"]
    ASSEMBLER.validate_spdx_subjects(
        spdx_path,
        subjects,
        version=version,
        manifest_sha256=ASSEMBLER.sha256_bytes(mirror_bytes),
    )
    ASSEMBLER.validate_provenance_subjects(provenance_path, subjects)
    try:
        IDENTITY.validate_identity_documents(
            spdx=ASSEMBLER.json_object(spdx_path, "SPDX SBOM"),
            provenance=ASSEMBLER.json_object(provenance_path, "SLSA provenance"),
            version=version,
            subjects_by_relative=subjects,
            payloads_by_id=payload_paths,
            require_github=os.environ.get("GITHUB_ACTIONS") == "true",
        )
    except IDENTITY.IdentityError as error:
        ASSEMBLER.fail(f"release supply-chain identity graph rejected: {error}")
    return manifest, inventory


def prepare_unsigned(
    release_dir: Path,
    payload_dir: Path,
    sources: dict[str, Path],
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    """Create and validate the exact pre-sign graph without an OIDC capability."""

    reject_oidc_environment("unsigned release aggregation")
    manifest, _authored, mirror_bytes = ASSEMBLER.load_manifest(
        manifest_toml, manifest_json
    )
    version, evidence_ids = ASSEMBLER.validate_release_contract(manifest)
    expected_sources = {spec.source_id for spec in source_specs()}
    if set(sources) != expected_sources:
        ASSEMBLER.fail(
            "pre-sign source reports must contain exactly every non-supply-chain source"
        )
    if release_dir.exists() and release_dir.is_symlink():
        ASSEMBLER.fail(f"release directory must not be a symlink: {release_dir}")
    release_dir.mkdir(parents=True, exist_ok=True)
    ASSEMBLER.validate_existing_release_dir(release_dir, version, evidence_ids)
    for relative in (
        ASSEMBLER.PRE_SIGN_SIGNATURE_PATH,
        ASSEMBLER.SIDECAR_PATHS["sigstore-bundle"],
    ):
        if (release_dir / relative).exists():
            ASSEMBLER.fail("unsigned aggregation must not contain a signature bundle")
    payload_paths = copy_payloads(release_dir, payload_dir, version)
    source_paths = copy_source_reports(release_dir, sources, manifest)
    ASSEMBLER.validate_release_payloads(
        payload_paths, source_paths, ASSEMBLER.sha256_bytes(mirror_bytes)
    )
    subjects = {
        path.relative_to(release_dir).as_posix(): path
        for path in (*payload_paths.values(), *source_paths.values())
    }
    inventory = inventory_value(version, payload_paths, source_paths, release_dir)
    inventory_path = release_dir / ASSEMBLER.PRE_SIGN_INVENTORY_PATH
    ASSEMBLER.atomic_write_bytes(
        inventory_path,
        ASSEMBLER.canonical_json(inventory),
        "pre-sign subject inventory",
    )
    ASSEMBLER.atomic_write_bytes(
        release_dir / ASSEMBLER.SIDECAR_PATHS["checksums"],
        ASSEMBLER.checksums_bytes(subjects),
        "SHA256SUMS",
    )
    created = now_iso()
    ASSEMBLER.atomic_write_bytes(
        release_dir / ASSEMBLER.SIDECAR_PATHS["spdx-sbom"],
        ASSEMBLER.canonical_json(
            generate_spdx(
                version,
                ASSEMBLER.sha256_bytes(mirror_bytes),
                subjects,
                created,
                payload_paths,
            )
        ),
        "pre-sign SPDX SBOM",
    )
    ASSEMBLER.atomic_write_bytes(
        release_dir / ASSEMBLER.SIDECAR_PATHS["slsa-provenance"],
        ASSEMBLER.canonical_json(
            generate_provenance(version, subjects, created, payload_paths)
        ),
        "pre-sign SLSA provenance",
    )
    ASSEMBLER.validate_spdx_subjects(
        release_dir / ASSEMBLER.SIDECAR_PATHS["spdx-sbom"],
        subjects,
        version=version,
        manifest_sha256=ASSEMBLER.sha256_bytes(mirror_bytes),
    )
    ASSEMBLER.validate_provenance_subjects(
        release_dir / ASSEMBLER.SIDECAR_PATHS["slsa-provenance"], subjects
    )
    try:
        IDENTITY.validate_identity_documents(
            spdx=ASSEMBLER.json_object(
                release_dir / ASSEMBLER.SIDECAR_PATHS["spdx-sbom"], "SPDX SBOM"
            ),
            provenance=ASSEMBLER.json_object(
                release_dir / ASSEMBLER.SIDECAR_PATHS["slsa-provenance"],
                "SLSA provenance",
            ),
            version=version,
            subjects_by_relative=subjects,
            payloads_by_id=payload_paths,
            require_github=os.environ.get("GITHUB_ACTIONS") == "true",
        )
    except IDENTITY.IdentityError as error:
        ASSEMBLER.fail(f"release supply-chain identity graph rejected: {error}")
    validate_prepared_material(release_dir, manifest_toml, manifest_json)


def write_signed_reports(
    release_dir: Path,
    producer_output: Path,
    source_output: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    """Verify the signed inventory and emit its deterministic typed reports."""

    manifest, inventory = validate_prepared_material(
        release_dir, manifest_toml, manifest_json
    )
    inventory_path = release_dir / ASSEMBLER.PRE_SIGN_INVENTORY_PATH
    signature_path = release_dir / ASSEMBLER.PRE_SIGN_SIGNATURE_PATH
    ASSEMBLER.verify_sigstore_signature(
        inventory_path, signature_path, "pre-sign subject inventory"
    )
    identity = os.environ.get("COSIGN_CERTIFICATE_IDENTITY", "").strip()
    issuer = os.environ.get("COSIGN_CERTIFICATE_OIDC_ISSUER", "").strip()
    if not identity or not issuer:
        ASSEMBLER.fail(
            "signed report emission requires the exact Sigstore identity policy"
        )
    producer = typed_producer(manifest, inventory, release_dir, identity, issuer)
    producer_output.parent.mkdir(parents=True, exist_ok=True)
    ASSEMBLER.atomic_write_bytes(
        producer_output,
        PRODUCER.canonical_json(producer),
        "typed supply-chain producer result",
    )
    producer_value = PRODUCER.read_producer(
        producer_output,
        PRODUCER.SOURCE_BY_ID[SUPPLY_CHAIN_SOURCE_ID],
        manifest,
    )
    producer_artifacts = {
        (SUPPLY_CHAIN_SOURCE_ID, binding): release_dir / relative
        for binding, relative in ARTIFACT_PATHS.items()
    }
    PRODUCER.verify_artifacts(
        SUPPLY_CHAIN_SOURCE_ID, producer_value, producer_artifacts
    )
    source_report = PRODUCER.source_report(
        PRODUCER.SOURCE_BY_ID[SUPPLY_CHAIN_SOURCE_ID],
        manifest,
        producer=producer_value,
    )
    source_output.parent.mkdir(parents=True, exist_ok=True)
    ASSEMBLER.atomic_write_bytes(
        source_output,
        PRODUCER.canonical_json(source_report),
        "typed supply-chain source report",
    )


def finalize_signed(
    release_dir: Path,
    producer_output: Path,
    source_output: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    """Finalize the typed supply-chain report in a no-OIDC job."""

    reject_oidc_environment("signed release finalization")
    write_signed_reports(
        release_dir,
        producer_output,
        source_output,
        manifest_toml,
        manifest_json,
    )


def produce(
    release_dir: Path,
    payload_dir: Path,
    sources: dict[str, Path],
    producer_output: Path,
    source_output: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    """Compatibility helper for local tests; release workflow uses split phases."""

    prepare_unsigned(
        release_dir,
        payload_dir,
        sources,
        manifest_toml,
        manifest_json,
    )
    sign_and_verify(
        release_dir / ASSEMBLER.PRE_SIGN_INVENTORY_PATH,
        release_dir / ASSEMBLER.PRE_SIGN_SIGNATURE_PATH,
    )
    write_signed_reports(
        release_dir,
        producer_output,
        source_output,
        manifest_toml,
        manifest_json,
    )


def verify_final(release_dir: Path, manifest_toml: Path, manifest_json: Path) -> None:
    manifest, _authored, mirror = ASSEMBLER.load_manifest(manifest_toml, manifest_json)
    version, evidence_ids = ASSEMBLER.validate_release_contract(manifest)
    metadata = ASSEMBLER.json_object(
        release_dir / "release-manifest.json", "release manifest"
    )
    evidence_metadata = metadata.get("evidence")
    if not isinstance(evidence_metadata, dict) or set(evidence_metadata) != set(
        evidence_ids
    ):
        ASSEMBLER.fail(
            "final manifest evidence map must contain exactly all "
            f"{ASSEMBLER.REQUIRED_RELEASE_EVIDENCE_COUNT} reports"
        )
    if evidence_metadata.get(SUPPLY_CHAIN_EVIDENCE_ID) != (
        f"evidence/{SUPPLY_CHAIN_EVIDENCE_ID}.json"
    ):
        ASSEMBLER.fail("final manifest supply-chain evidence path is not deterministic")
    payload_paths = {
        artifact_id: release_dir / relative
        for artifact_id, relative in ASSEMBLER.payload_names(version).items()
    }
    evidence_paths = {
        evidence_id: release_dir / f"evidence/{evidence_id}.json"
        for evidence_id in evidence_ids
    }
    ASSEMBLER.validate_normalized_evidence_reports(
        evidence_paths, evidence_ids, version, manifest["contracts"]
    )
    ASSEMBLER.validate_pre_sign_material(
        release_dir,
        payload_paths,
        evidence_paths,
        evidence_ids,
        version,
        manifest["contracts"],
        ASSEMBLER.sha256_bytes(mirror),
    )
    inventory = ASSEMBLER.json_object(
        release_dir / ASSEMBLER.PRE_SIGN_INVENTORY_PATH, "pre-sign subject inventory"
    )
    inventory_subjects = {
        item["id"]: item for item in inventory["subjects"] if item["kind"] == "payload"
    }
    artifacts = metadata.get("artifacts")
    artifact_digests = metadata.get("artifact_digests")
    expected_artifact_ids = set(ASSEMBLER.RELEASE_ARTIFACT_IDS)
    if (
        not isinstance(artifacts, dict)
        or set(artifacts) != expected_artifact_ids
        or not isinstance(artifact_digests, dict)
        or set(artifact_digests) != expected_artifact_ids - {"sigstore-bundle"}
    ):
        ASSEMBLER.fail("final manifest artifact maps are incomplete or circular")
    for artifact_id, relative in artifacts.items():
        if not isinstance(relative, str) or relative != (
            ASSEMBLER.payload_names(version).get(artifact_id)
            or ASSEMBLER.SIDECAR_PATHS.get(artifact_id)
        ):
            ASSEMBLER.fail(
                f"final manifest artifact path is not deterministic: {artifact_id}"
            )
        if artifact_id != "sigstore-bundle":
            observed = "sha256:" + ASSEMBLER.sha256_file(release_dir / relative)
            if artifact_digests.get(artifact_id) != observed:
                ASSEMBLER.fail(
                    f"final manifest artifact digest mismatch: {artifact_id}"
                )
    for artifact_id, subject in inventory_subjects.items():
        if artifact_digests.get(artifact_id) != subject["sha256"]:
            ASSEMBLER.fail(
                "final manifest payload digest is not bound to signed subject "
                f"inventory: {artifact_id}"
            )
    supply_report = ASSEMBLER.json_object(
        evidence_paths[SUPPLY_CHAIN_EVIDENCE_ID], "normalized supply-chain evidence"
    )
    if supply_report.get("evidence_id") != SUPPLY_CHAIN_EVIDENCE_ID:
        ASSEMBLER.fail("final supply-chain evidence ID is invalid")
    evidence_digests = metadata.get("evidence_digests")
    if not isinstance(evidence_digests, dict) or set(evidence_digests) != set(
        evidence_ids
    ):
        ASSEMBLER.fail("final manifest evidence digest map is incomplete")
    for evidence_id, path in evidence_paths.items():
        if evidence_digests[evidence_id] != "sha256:" + ASSEMBLER.sha256_file(path):
            ASSEMBLER.fail(f"final manifest evidence digest mismatch: {evidence_id}")
    pre_sign_subjects = {
        item["path"]: release_dir / item["path"] for item in inventory["subjects"]
    }
    try:
        IDENTITY.validate_identity_documents(
            spdx=ASSEMBLER.json_object(
                release_dir / ASSEMBLER.SIDECAR_PATHS["spdx-sbom"], "SPDX SBOM"
            ),
            provenance=ASSEMBLER.json_object(
                release_dir / ASSEMBLER.SIDECAR_PATHS["slsa-provenance"],
                "SLSA provenance",
            ),
            version=version,
            subjects_by_relative=pre_sign_subjects,
            payloads_by_id=payload_paths,
            require_github=True,
        )
    except IDENTITY.IdentityError as error:
        ASSEMBLER.fail(f"final release supply-chain identity graph rejected: {error}")
    ASSEMBLER.verify_release_signatures(release_dir)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--release-dir", type=Path, required=True)
    command.add_argument("--payload-dir", type=Path)
    command.add_argument(
        "--source", action="append", default=[], metavar="SOURCE_ID=PATH"
    )
    command.add_argument("--producer-output", type=Path)
    command.add_argument("--source-output", type=Path)
    phase = command.add_mutually_exclusive_group()
    phase.add_argument("--prepare-unsigned", action="store_true")
    phase.add_argument("--finalize-signed", action="store_true")
    phase.add_argument("--verify", action="store_true")
    command.add_argument(
        "--manifest-toml", type=Path, default=ASSEMBLER.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ASSEMBLER.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.verify:
            verify_final(args.release_dir, args.manifest_toml, args.manifest_json)
        elif args.prepare_unsigned:
            if not args.payload_dir or args.producer_output or args.source_output:
                ASSEMBLER.fail(
                    "unsigned preparation requires --payload-dir and forbids producer outputs"
                )
            sources = parse_sources(
                args.source,
                {spec.source_id for spec in source_specs()},
            )
            prepare_unsigned(
                args.release_dir,
                args.payload_dir,
                sources,
                args.manifest_toml,
                args.manifest_json,
            )
        elif args.finalize_signed:
            if (
                args.payload_dir
                or args.source
                or not args.producer_output
                or not args.source_output
            ):
                ASSEMBLER.fail(
                    "signed finalization requires both producer outputs and no payload/source inputs"
                )
            finalize_signed(
                args.release_dir,
                args.producer_output,
                args.source_output,
                args.manifest_toml,
                args.manifest_json,
            )
        else:
            if (
                not args.payload_dir
                or not args.producer_output
                or not args.source_output
            ):
                ASSEMBLER.fail(
                    "generation requires --payload-dir, --source-output, and --producer-output"
                )
            sources = parse_sources(
                args.source,
                {spec.source_id for spec in source_specs()},
            )
            produce(
                args.release_dir,
                args.payload_dir,
                sources,
                args.producer_output,
                args.source_output,
                args.manifest_toml,
                args.manifest_json,
            )
    except ASSEMBLER.AssemblyError as error:
        print(f"release supply-chain operation failed: {error}", file=sys.stderr)
        return 1
    print("release supply-chain operation completed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
