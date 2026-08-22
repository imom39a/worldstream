"""Focused tests for typed release evidence producer adapters."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
PRODUCE = ROOT / "scripts/release-evidence-produce.py"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture()
def producer(tmp_path: Path):
    module = load_module("release_evidence_produce", PRODUCE)
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_json = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", manifest_toml)
    shutil.copy2(ROOT / "compatibility.json", manifest_json)
    return module, manifest_toml, manifest_json, tmp_path


def producer_value(
    module, manifest_toml: Path, source_id: str, artifact: Path | None = None
):
    manifest = module.COLLECTOR.load_manifest(
        manifest_toml, manifest_toml.with_name("compatibility.json")
    )
    spec = module.SOURCE_BY_ID[source_id]
    artifacts = {}
    if artifact is not None:
        data = artifact.read_bytes()
        binding_id = module.REQUIRED_ARTIFACT_BINDINGS[source_id][0]
        artifacts[binding_id] = {
            "sha256": "sha256:" + hashlib.sha256(data).hexdigest(),
            "size_bytes": len(data),
        }
    return {
        "schema": module.PRODUCER_SCHEMA,
        "producer_id": module.COLLECTOR.EXPECTED_PRODUCER_IDS[source_id],
        "evidence_id": spec.evidence_id,
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": module.COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "passed",
                "observations": [{"kind": "typed-observation", "value": check}],
            }
            for check in spec.checks
        },
        "artifacts": artifacts,
    }


def write_producer(
    module, manifest_toml: Path, root: Path, source_id: str, value: dict
):
    path = root / f"{source_id}.json"
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def test_typed_producer_and_artifact_are_promoted_only_after_byte_validation(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"actual release subject\n")
    source_id = "native-linux"
    value = producer_value(module, manifest_toml, source_id, artifact)
    producer_path = write_producer(module, manifest_toml, tmp_path, source_id, value)

    evidence_ids, failed = module.produce(
        tmp_path / "source-reports",
        {source_id: producer_path},
        {
            spec.source_id: "not supplied in focused fixture"
            for spec in module.SOURCE_SPECS
            if spec.source_id != source_id
        },
        {(source_id, module.REQUIRED_ARTIFACT_BINDINGS[source_id][0]): artifact},
        manifest_toml,
        manifest_json,
    )

    assert len(evidence_ids) == module.COLLECTOR.REQUIRED_RELEASE_EVIDENCE_COUNT
    assert "native-linux" not in failed
    report = json.loads(
        (tmp_path / "source-reports" / "native-linux-release-profile.json").read_text()
    )
    assert report["release_evidence"] is True
    assert report["checks"] == {
        check: True for check in module.SOURCE_BY_ID[source_id].checks
    }
    assert (
        report["details"]["artifacts"][module.REQUIRED_ARTIFACT_BINDINGS[source_id][0]][
            "sha256"
        ]
        == value["artifacts"][module.REQUIRED_ARTIFACT_BINDINGS[source_id][0]]["sha256"]
    )


def test_tampered_artifact_is_fail_closed(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"actual bytes")
    value = producer_value(module, manifest_toml, "native-linux", artifact)
    artifact.write_bytes(b"tampered bytes")
    producer_path = write_producer(
        module, manifest_toml, tmp_path, "native-linux", value
    )
    missing = {
        spec.source_id: "not supplied in focused fixture"
        for spec in module.SOURCE_SPECS
        if spec.source_id != "native-linux"
    }

    _ids, failed = module.produce(
        tmp_path / "source-reports",
        {"native-linux": producer_path},
        missing,
        {
            (
                "native-linux",
                module.REQUIRED_ARTIFACT_BINDINGS["native-linux"][0],
            ): artifact
        },
        manifest_toml,
        manifest_json,
    )

    assert "native-linux" in failed
    report = json.loads(
        (tmp_path / "source-reports" / "native-linux-release-profile.json").read_text()
    )
    assert report["release_evidence"] is False
    assert report["fail_closed"] is True
    assert "digest mismatch" in report["details"]["reason"]


def test_explicit_release_evidence_false_cannot_be_promoted(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"release subject")
    value = producer_value(module, manifest_toml, "native-linux", artifact)
    value["release_evidence"] = False
    producer_path = write_producer(
        module, manifest_toml, tmp_path, "native-linux", value
    )
    missing = {
        spec.source_id: "not supplied in focused fixture"
        for spec in module.SOURCE_SPECS
        if spec.source_id != "native-linux"
    }

    _ids, failed = module.produce(
        tmp_path / "source-reports",
        {"native-linux": producer_path},
        missing,
        {
            (
                "native-linux",
                module.REQUIRED_ARTIFACT_BINDINGS["native-linux"][0],
            ): artifact
        },
        manifest_toml,
        manifest_json,
    )

    assert "native-linux" in failed
    report = json.loads(
        (tmp_path / "source-reports" / "native-linux-release-profile.json").read_text()
    )
    assert report["release_evidence"] is False
    assert report["fail_closed"] is True
    assert "not an explicit passed release attestation" in report["details"]["reason"]


def test_boolean_map_exit_code_only_and_tampered_schema_fail_closed(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    value = producer_value(module, manifest_toml, "native-linux")
    value["checks"] = {
        check: True for check in module.SOURCE_BY_ID["native-linux"].checks
    }
    value.pop("outcomes")
    value["exit_code"] = 0
    producer_path = write_producer(
        module, manifest_toml, tmp_path, "native-linux", value
    )
    missing = {
        spec.source_id: "not supplied in focused fixture"
        for spec in module.SOURCE_SPECS
        if spec.source_id != "native-linux"
    }

    _ids, failed = module.produce(
        tmp_path / "source-reports",
        {"native-linux": producer_path},
        missing,
        {},
        manifest_toml,
        manifest_json,
    )

    assert "native-linux" in failed
    report = json.loads(
        (tmp_path / "source-reports" / "native-linux-release-profile.json").read_text()
    )
    assert report["release_evidence"] is False
    assert "wrong fields" in report["details"]["reason"]


def test_missing_producer_is_named_and_fail_closed(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    missing = {
        spec.source_id: f"missing producer: {spec.source_id}-release-adapter"
        for spec in module.SOURCE_SPECS
    }

    _ids, failed = module.produce(
        tmp_path / "source-reports",
        {},
        missing,
        {},
        manifest_toml,
        manifest_json,
    )

    assert len(failed) == module.COLLECTOR.REQUIRED_RELEASE_EVIDENCE_COUNT
    report = json.loads(
        (tmp_path / "source-reports" / "native-linux-release-profile.json").read_text()
    )
    assert report["status"] == "unavailable"
    assert report["release_evidence"] is False
    assert report["fail_closed"] is True
    assert report["details"]["reason"] == (
        "missing producer: native-linux-release-adapter"
    )


@pytest.mark.parametrize("bad_status", [[], {}, 1])
def test_unhashable_or_non_string_producer_status_is_rejected(producer, bad_status):
    module, manifest_toml, _manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"release subject")
    value = producer_value(module, manifest_toml, "native-linux", artifact)
    value["status"] = bad_status
    path = write_producer(module, manifest_toml, tmp_path, "native-linux", value)

    with pytest.raises(module.ProducerError, match="producer status must be a string"):
        module.read_producer(
            path,
            module.SOURCE_BY_ID["native-linux"],
            module.COLLECTOR.load_manifest(
                manifest_toml, manifest_toml.with_name("compatibility.json")
            ),
        )


@pytest.mark.parametrize("bad_status", [[], {}, 1])
def test_unhashable_or_non_string_outcome_status_is_rejected(producer, bad_status):
    module, manifest_toml, _manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"release subject")
    value = producer_value(module, manifest_toml, "native-linux", artifact)
    value["outcomes"]["archive_identity"]["status"] = bad_status
    path = write_producer(module, manifest_toml, tmp_path, "native-linux", value)

    with pytest.raises(
        module.ProducerError,
        match="producer outcome archive_identity status must be a string",
    ):
        module.read_producer(
            path,
            module.SOURCE_BY_ID["native-linux"],
            module.COLLECTOR.load_manifest(
                manifest_toml, manifest_toml.with_name("compatibility.json")
            ),
        )


def test_required_artifact_binding_cannot_be_omitted(producer):
    module, manifest_toml, _manifest_json, tmp_path = producer
    value = producer_value(module, manifest_toml, "native-linux")
    path = write_producer(module, manifest_toml, tmp_path, "native-linux", value)

    with pytest.raises(module.ProducerError, match="artifact bindings"):
        module.read_producer(
            path,
            module.SOURCE_BY_ID["native-linux"],
            module.COLLECTOR.load_manifest(
                manifest_toml, manifest_toml.with_name("compatibility.json")
            ),
        )


def test_artifact_binding_cannot_be_attached_to_missing_producer(producer):
    module, manifest_toml, manifest_json, tmp_path = producer
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"diagnostic only")
    missing = {
        spec.source_id: "not supplied in focused fixture"
        for spec in module.SOURCE_SPECS
    }

    with pytest.raises(module.ProducerError, match="without producers"):
        module.produce(
            tmp_path / "source-reports",
            {},
            missing,
            {
                (
                    "native-linux",
                    module.REQUIRED_ARTIFACT_BINDINGS["native-linux"][0],
                ): artifact
            },
            manifest_toml,
            manifest_json,
        )


def test_workflow_maps_typed_producers_and_exact_artifact_bindings():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    release = workflow[workflow.index("  release-evidence:") :]
    for source_id in (
        "manifest-contract",
        "sqlite-conformance",
        "postgres-conformance",
        "migration-history",
        "transfer",
        "restore",
        "native-linux",
        "native-windows",
        "oci-linux",
        "macos-source",
        "security-observability",
        "failure-soak",
        "reference-performance",
    ):
        assert f"--producer {source_id}=" in release
        assert f"--source {source_id}=release-inputs/source-reports/" in release
        assert f"--missing {source_id}=" not in release
    for binding in (
        "manifest-contract/compatibility-pair",
        "sqlite-conformance/sqlite-conformance-result",
        "postgres-conformance/postgres-conformance-result",
        "migration-history/migration-history-result",
        "transfer/transfer-result",
        "restore/restore-result",
        "native-linux/linux-release-profile",
        "native-windows/windows-release-profile",
        "oci-linux/oci-release-profile",
        "macos-source/macos-quickstart",
        "security-observability/security-observability",
        "failure-soak/failure-soak",
        "reference-performance/reference-performance",
    ):
        assert f"--artifact {binding}=" in release
    assert (
        "--missing supply-chain=supply-chain-pre-sign-typed-producer-not-uploaded"
        in release
    )
    assert "--missing failure-soak=" not in release
    assert (
        "--producer failure-soak=release-inputs/failure/producers/failure-soak.json"
        in release
    )
    assert "release-inputs/failure/artifacts/failure-soak.json" in release
    assert "pattern: release-platform-producer-*" in release
    assert "name: release-conformance-producers" in release
    assert (
        "COSIGN_CERTIFICATE_IDENTITY: https://github.com/imom39a/worldstream/"
        in release
    )
    assert (
        "COSIGN_CERTIFICATE_OIDC_ISSUER: https://token.actions.githubusercontent.com"
        in release
    )
    assert "Produce and verify unsigned subject inventory" in release
    assert "Sign exact pre-sign subject inventory" in release
    assert "Assemble exact unsigned final manifest" in release
    assert "Sign exact detached final manifest" in release
    assert "Verify both detached release signatures and all subjects" in release
    assert "supply-chain-pre-sign-typed-producer-not-uploaded" in release
    assert "supply-chain-producer-requires-final-subjects" not in release


def test_workflow_hard_requires_packaged_failure_soak_artifact_layout():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    failure = workflow[
        workflow.index("  failure-soak-release:") : workflow.index(
            "  release-evidence:"
        )
    ]
    release = workflow[workflow.index("  release-evidence:") :]

    assert (
        "needs: [fast, native, macos-source-release, conformance-release, "
        "packaged-backend-release, failure-soak-release, "
        "reference-performance-release]" in release
    )
    assert "name: release-native-linux" in failure
    assert "name: release-linux-package-report" in failure
    assert "needs: [native, packaged-backend-release]" in failure
    assert (
        "runs-on: [self-hosted, linux, x64, "
        "ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd]" in failure
    )
    assert "name: release-packaged-backend-parity" in failure
    assert "scripts/kill-point-smoke.sh" in failure
    assert "--output reports/kill-point-evidence.json" in failure
    assert "scripts/daemon-transition-soak.sh" in failure
    assert "--one-hour" in failure
    assert (
        "--packaged-acceptance-report "
        "package-acceptance/postgres-packaged-acceptance.json" in failure
    )
    assert "--output reports/daemon-transition-soak.json" in failure
    assert "scripts/release-evidence-produce-failure-soak.py" in failure
    assert "--package-archive '${{ steps.packaged_linux.outputs.archive }}'" in failure
    assert "--package-report '${{ steps.packaged_linux.outputs.report }}'" in failure
    assert "--daemon-bin '${{ steps.packaged_linux.outputs.daemon }}'" in failure
    assert "name: release-failure-soak" in failure
    assert "path: release-inputs/failure/" in failure
    assert "name: release-failure-soak" in release
    assert "path: release-inputs/failure" in release


def test_reference_measurement_jobs_require_the_frozen_certified_host_label():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    label = (
        "runs-on: [self-hosted, linux, x64, "
        "ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd]"
    )
    packaged = workflow[
        workflow.index("  packaged-backend-release:") : workflow.index(
            "  failure-soak-release:"
        )
    ]
    failure = workflow[
        workflow.index("  failure-soak-release:") : workflow.index(
            "  reference-performance-release:"
        )
    ]
    reference = workflow[
        workflow.index("  reference-performance-release:") : workflow.index(
            "  release-evidence:"
        )
    ]
    assert label in packaged
    assert label in failure
    assert label in reference
    assert "runs-on: ubuntu-24.04" not in packaged
    assert "runs-on: ubuntu-24.04" not in failure
    assert "runs-on: ubuntu-24.04" not in reference


def test_workflow_projects_and_uploads_exact_packaged_reference_inputs():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    reference = workflow[
        workflow.index("  reference-performance-release:") : workflow.index(
            "  release-evidence:"
        )
    ]
    release = workflow[workflow.index("  release-evidence:") :]

    assert (
        "needs: [native, packaged-backend-release, failure-soak-release]" in reference
    )
    assert "always()" not in reference[: reference.index("    steps:")]
    for artifact_name, destination in (
        ("release-native-linux", "package-input"),
        ("release-linux-package-report", "package-input"),
        ("release-packaged-backend-parity", "package-acceptance"),
        ("release-failure-soak-diagnostics", "process-input"),
    ):
        assert f"name: {artifact_name}" in reference
        assert f"path: {destination}" in reference
    for exact_path in (
        "package-input/worldstream-*-linux-x86_64.tar.gz",
        "package-input/native-linux-package.json",
        "package-acceptance/postgres-packaged-acceptance.json",
        "process-input/daemon-transition-soak.json",
    ):
        assert exact_path in reference
    assert "scripts/release-package-extract.py" in reference
    assert "mkdir -p package-extracted reports reference-inputs/normalized" in reference
    assert "scripts/reference-evidence-project.py" in reference
    assert "--output-dir reference-inputs/normalized" in reference
    assert "--aggregate-report reference-inputs/reference-performance.json" in reference
    assert "scripts/release-evidence-produce-reference.py" in reference
    for kind in ("counter", "heist", "sqlite", "postgres", "soak"):
        assert f"--{kind}-report reference-inputs/normalized/{kind}.json" in reference
    assert "name: release-reference-performance" in reference
    assert "path: release-inputs/reference/" in reference
    header = release[: release.index("    steps:")]
    assert "reference-performance-release" in header
    assert "always()" not in header


def test_release_job_cannot_run_after_a_failed_required_producer_job():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    release = workflow[workflow.index("  release-evidence:") :]
    header = release[: release.index("    steps:")]
    assert "always()" not in header
    assert "conformance-release" in header
    assert "failure-soak-release" in header
    assert "reference-performance-release" in header
    assert "macos-source-release" in header


def test_workflow_requires_both_macos_architectures_before_typed_release_evidence():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    raw = workflow[
        workflow.index("  macos-source:") : workflow.index("  macos-source-release:")
    ]
    typed = workflow[
        workflow.index("  macos-source-release:") : workflow.index(
            "  conformance-release:"
        )
    ]
    release = workflow[workflow.index("  release-evidence:") :]

    assert "runner: macos-15" in raw
    assert "architecture: arm64" in raw
    assert "runner: macos-15-intel" in raw
    assert "architecture: x86_64" in raw
    assert "timeout-minutes: 30" in raw
    assert "scripts/install-pinned-browser.py" in raw
    assert "--browser-archive-sha256" in raw
    assert "release-macos-source-raw-${{ matrix.architecture }}" in raw
    assert "needs: [macos-source]" in typed
    assert "pattern: release-macos-source-raw-*" in typed
    assert "macos-source-arm64.json" in typed
    assert "macos-source-x86_64.json" in typed
    assert "--artifact-output reports/macos-source-matrix.json" in typed
    assert "macos-source-release" in release[: release.index("    steps:")]
    assert "release-inputs/macos/macos-source-matrix.json" in release


def test_source_archive_is_built_from_clean_checkout_before_tools_or_downloads():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    release = workflow[workflow.index("  release-evidence:") :]
    checkout = release.index("actions/checkout@")
    source = release.index("Build and verify source archive from the clean checkout")
    download = release.index("Download native and OCI release artifacts")
    assert checkout < source < download
    source_step = release[source:download]
    assert "--source-dir ." in source_step
    assert "$RUNNER_TEMP/worldstream-clean-source-payload" in source_step
    assert "release-inputs" not in source_step
    assert source_step.count("--target source") == 2


def test_workflow_download_layout_matches_every_typed_source_map():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    release = workflow[workflow.index("  release-evidence:") :]
    downloads = {
        "platform": (
            "pattern: release-platform-producer-*",
            "path: release-inputs/platform-producers",
            "release-inputs/platform-producers/release-producers/",
        ),
        "macos": (
            "name: release-macos-source-evidence",
            "path: release-inputs/macos",
            "release-inputs/macos/release-producers/",
        ),
        "conformance": (
            "name: release-conformance-producers",
            "path: release-inputs/conformance",
            "release-inputs/conformance/producers/",
        ),
        "failure": (
            "name: release-failure-soak",
            "path: release-inputs/failure",
            "release-inputs/failure/producers/",
        ),
        "reference": (
            "name: release-reference-performance",
            "path: release-inputs/reference",
            "release-inputs/reference/producers/",
        ),
    }
    for artifact_name, download_path, consumed_root in downloads.values():
        assert artifact_name in release
        assert download_path in release
        assert consumed_root in release

    assert "merge-multiple: true" in release
    assert "release-inputs/platform-producers/telemetry-https.json" in release


def test_workflow_sequences_pre_sign_supply_chain_without_a_source_cycle():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    release = workflow[workflow.index("  release-evidence:") :]
    adapt = release.index(
        '"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/release-evidence-produce.py'
    )
    supply = release.index(
        '"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/release-supply-chain.py'
    )
    collect = release.index(
        '"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/release-evidence-collect.py'
    )
    assert adapt < supply < collect
    adapter_step = release[adapt:supply]
    assert "--missing supply-chain=" in adapter_step
    assert "--producer supply-chain=" not in adapter_step
    assert (
        "--source-output release-inputs/source-reports/"
        "checksums-signature-sbom-provenance.json"
    ) in release[supply:collect]


def test_workflow_orders_pre_sign_inventory_before_collection_and_final_signing():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    unsigned = workflow.index("  release-evidence:")
    inventory_sign = workflow.index("  release-signing:")
    assemble = workflow.index("  release-finalize:")
    manifest_sign = workflow.index("  release-manifest-signing:")
    verify = workflow.index("  release-verify:")
    assert unsigned < inventory_sign < assemble < manifest_sign < verify

    unsigned_block = workflow[unsigned:inventory_sign]
    inventory_sign_block = workflow[inventory_sign:assemble]
    assemble_block = workflow[assemble:manifest_sign]
    manifest_sign_block = workflow[manifest_sign:verify]
    verify_block = workflow[verify:]
    assert "--prepare-unsigned" in unsigned_block
    assert 'test "$(find dist -type f | wc -l)" -eq 21' in unsigned_block
    assert "layout_sha256=" in unsigned_block
    assert "cosign sign-blob" not in unsigned_block
    assert "cosign sign-blob" in inventory_sign_block
    assert '" = "$EXPECTED_LAYOUT_SHA256"' in inventory_sign_block
    assert "--finalize-signed" in assemble_block
    assert "Assemble detached v2 release manifest" in assemble_block
    assert 'test "$(find dist -type f | wc -l)" -eq 37' in assemble_block
    assert "layout_sha256=" in assemble_block
    assert "cosign sign-blob" not in assemble_block
    assert "cosign sign-blob" in manifest_sign_block
    assert '" = "$EXPECTED_LAYOUT_SHA256"' in manifest_sign_block
    assert "scripts/verify-release.sh dist" in verify_block
    assert 'test "$(find dist -type f | wc -l)" -eq 38' in verify_block


def test_workflow_confines_oidc_to_two_minimal_main_bound_signing_jobs():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    boundaries = {
        name: workflow.index(f"  {name}:")
        for name in (
            "release-evidence",
            "release-signing",
            "release-finalize",
            "release-manifest-signing",
            "release-verify",
        )
    }
    ordered = sorted(boundaries, key=boundaries.get)
    blocks = {}
    for index, name in enumerate(ordered):
        start = boundaries[name]
        end = (
            boundaries[ordered[index + 1]]
            if index + 1 < len(ordered)
            else len(workflow)
        )
        blocks[name] = workflow[start:end]

    assert workflow.count("      id-token: write") == 2
    for name in ("release-signing", "release-manifest-signing"):
        block = blocks[name]
        assert "    environment: worldstream-release" in block
        assert "      id-token: write" in block
        assert "actions/checkout@" not in block
        assert "setup-uv@" not in block
        assert "setup-node@" not in block
        assert "cargo " not in block
        assert "pnpm " not in block
        assert "github.ref == 'refs/heads/main'" in block
    for name in ("release-evidence", "release-finalize", "release-verify"):
        block = blocks[name]
        assert "id-token:" not in block
        assert "github.ref == 'refs/heads/main'" in block
    for name, next_name in (
        ("packaged-backend-release", "failure-soak-release"),
        ("failure-soak-release", "reference-performance-release"),
        ("reference-performance-release", "release-evidence"),
    ):
        start = workflow.index(f"  {name}:")
        end = workflow.index(f"  {next_name}:", start)
        block = workflow[start:end]
        assert "    environment: worldstream-release" in block
        assert "github.ref == 'refs/heads/main'" in block
