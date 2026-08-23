"""Focused tests for detached release assembly and subject consistency."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ASSEMBLE = ROOT / "scripts/release-evidence-assemble.py"
SUPPLY_CHAIN = ROOT / "scripts/release-supply-chain.py"
COLLECT = ROOT / "scripts/release-evidence-collect.py"
PACKAGE = ROOT / "scripts/package.py"
VERIFY_RELEASE_SH = ROOT / "scripts/verify-release.sh"
VERIFY_RELEASE_PS1 = ROOT / "scripts/verify-release.ps1"
PACKAGE_SMOKE = ROOT / "tests/package_smoke.py"
OCI_RUNTIME_SMOKE = ROOT / "tests/oci_runtime_smoke.py"
PAYLOAD_BYTES_CACHE: dict[str, bytes] | None = None


def hosted_build_environment(identity, target: str) -> dict:
    runner = {
        "provider": "github-actions",
        **identity.HOSTED_RUNNER_FACTS[target],
        "image_version": "20260817.1.0",
    }
    if target == "source":
        return {
            "runner": runner,
            "rustc": None,
            "bundled_sqlite": None,
            "final_linker": None,
        }
    windows = target == "windows-x64"
    root = "C:\\hostedtoolcache\\fixture" if windows else "/opt/hostedtoolcache/fixture"
    return {
        "runner": runner,
        "rustc": {
            "path": f"{root}/rustc.exe" if windows else f"{root}/rustc",
            "version": "rustc 1.97.1 (fixture)",
            "target": identity.TARGET_TRIPLES[target],
            "reported_target": "x86_64-pc-windows-msvc"
            if windows
            else "x86_64-unknown-linux-gnu",
        },
        "bundled_sqlite": {
            "archiver": {
                "path": f"{root}/Hostx64/x64/lib.exe"
                if windows
                else f"{root}/zig-musl-ar",
                "version": "fixture archiver 1.0",
                "target": identity.TARGET_TRIPLES[target],
                "reported_target": ("x64-coff-library" if windows else "gnu-archive"),
            },
            "c_compiler": {
                "path": f"{root}/cl.exe" if windows else f"{root}/cc",
                "version": "fixture C compiler 1.0",
                "target": identity.TARGET_TRIPLES[target],
                "reported_target": ("x64" if windows else "x86_64-unknown-linux-musl"),
            },
        },
        "final_linker": {
            "path": f"{root}/Hostx64/x64/link.exe" if windows else f"{root}/rust-lld",
            "version": "fixture linker 1.0",
            "target": identity.TARGET_TRIPLES[target],
            "reported_target": "x64" if windows else "elf_x86_64",
        },
    }


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def manifest() -> dict:
    return json.loads((ROOT / "compatibility.json").read_text(encoding="utf-8"))


def payload_names(version: str) -> dict[str, str]:
    return {
        "source-archive": f"worldstream-{version}-source.tar.gz",
        "native-linux-x86_64-archive": f"worldstream-{version}-linux-x86_64.tar.gz",
        "native-windows-x64-archive": f"worldstream-{version}-windows-x64.zip",
        "oci-linux-amd64-image": f"worldstream-{version}-oci-linux-amd64.oci.tar",
    }


def valid_payload_bytes(
    tmp_path: Path, value: dict, *, authored_manifest: bytes | None = None
) -> dict[str, bytes]:
    global PAYLOAD_BYTES_CACHE
    cacheable = authored_manifest is None and value == manifest()
    if cacheable and PAYLOAD_BYTES_CACHE is not None:
        return PAYLOAD_BYTES_CACHE

    helpers = load_module("release_assembly_package_helpers", PACKAGE_SMOKE)
    package = helpers.PACKAGE
    version = value["release_candidate"]
    manifest_toml = authored_manifest or (ROOT / "compatibility.toml").read_bytes()
    manifest_json = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    build_root = tmp_path / "valid-payload-builder"
    native_root = build_root / "native"
    inputs = helpers.fixture_inputs(native_root)
    helpers.add_fixture_client_identities(native_root, inputs, value)
    heist_pack = next(
        row
        for row in package.canonical_client_contract_identity(value)["pack_executors"]
        if row["pack_id"] == "worldstream.agent-heist"
    )
    heist_fixture = (
        json.dumps(
            {
                "retained_executor": {
                    "pack_id": heist_pack["pack_id"],
                    "pack_version": heist_pack["explanatory_version"],
                    "pack_digest": heist_pack["revision_digest"],
                }
            },
            sort_keys=True,
        ).encode()
        + b"\n"
    )
    (native_root / "examples/heist/parity_fixture.json").write_bytes(heist_fixture)

    linux_files = package.collect_package_files(
        package.TARGETS["linux-x86_64"],
        version,
        manifest_toml,
        manifest_json,
        inputs,
        0,
        observed_build_environment=hosted_build_environment(
            package.BUILD_IDENTITY, "linux-x86_64"
        ),
        require_hosted_environment=True,
        require_clean_checkout=False,
    )
    linux_path = build_root / payload_names(version)["native-linux-x86_64-archive"]
    package.write_tar_gz(
        linux_path, f"worldstream-{version}-linux-x86_64", linux_files, 0
    )

    windows_inputs = {group: list(entries) for group, entries in inputs.items()}
    windows_inputs["bin"] = []
    for binary in package.TARGETS["windows-x64"].binary_names:
        source = native_root / binary
        source.write_bytes(f"windows fixture {binary}\n".encode())
        windows_inputs["bin"].append((source, f"bin/{binary}"))
    windows_files = package.collect_package_files(
        package.TARGETS["windows-x64"],
        version,
        manifest_toml,
        manifest_json,
        windows_inputs,
        0,
        observed_build_environment=hosted_build_environment(
            package.BUILD_IDENTITY, "windows-x64"
        ),
        require_hosted_environment=True,
        require_clean_checkout=False,
    )
    windows_path = build_root / payload_names(version)["native-windows-x64-archive"]
    package.write_zip(
        windows_path, f"worldstream-{version}-windows-x64", windows_files, 0
    )

    source_workspace, _source_inputs = helpers.wrapper_workspace(
        build_root / "source", manifest_toml, manifest_json
    )
    (source_workspace / "examples/heist/parity_fixture.json").write_bytes(heist_fixture)
    source_inputs = package.required_inputs(
        package.TARGETS["source"],
        source_workspace,
        source_workspace,
        source_workspace,
        source_workspace,
        source_workspace,
        source_dir=source_workspace,
    )
    source_files = package.collect_package_files(
        package.TARGETS["source"],
        version,
        manifest_toml,
        manifest_json,
        source_inputs,
        0,
        observed_build_environment=hosted_build_environment(
            package.BUILD_IDENTITY, "source"
        ),
        require_hosted_environment=True,
        require_clean_checkout=False,
    )
    source_path = build_root / payload_names(version)["source-archive"]
    package.write_tar_gz(source_path, f"worldstream-{version}-source", source_files, 0)

    oci_helpers = load_module("release_assembly_oci_helpers", OCI_RUNTIME_SMOKE)
    oci_fixture_root = build_root / "oci"
    oci_fixture_root.mkdir()
    source_entries = package.BUILD_IDENTITY.source_entries_from_root(ROOT)
    source_entries["compatibility.toml"] = manifest_toml
    source_entries["compatibility.json"] = manifest_json
    revision = package.BUILD_IDENTITY.source_revision(ROOT)
    base_image = package.BUILD_IDENTITY.expected_base_image(source_entries)
    oci_identity = package.BUILD_IDENTITY.build_identity(
        target="oci-linux-amd64",
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=0,
        manifest_sha256=hashlib.sha256(manifest_json).hexdigest(),
        base_image=base_image,
        observed_build_environment=hosted_build_environment(
            package.BUILD_IDENTITY, "oci-linux-amd64"
        ),
    )
    synthetic_oci, _config_digest = oci_helpers.oci_layout_fixture(
        oci_fixture_root,
        labels={
            "org.opencontainers.image.version": version,
            "org.opencontainers.image.revision": revision,
            "io.worldstream.target": "linux/amd64",
            "io.worldstream.base-image": base_image,
            "io.worldstream.build-identity": package.BUILD_IDENTITY.build_identity_digest(
                oci_identity
            ),
            "io.worldstream.build-environment": package.BUILD_IDENTITY.observed_build_environment_label(
                oci_identity["observed_build_environment"]
            ),
        },
    )
    oci_path = build_root / payload_names(version)["oci-linux-amd64-image"]
    shutil.copy2(synthetic_oci, oci_path)

    generated = {
        "source-archive": source_path.read_bytes(),
        "native-linux-x86_64-archive": linux_path.read_bytes(),
        "native-windows-x64-archive": windows_path.read_bytes(),
        "oci-linux-amd64-image": oci_path.read_bytes(),
    }
    for artifact_id, path in (
        ("source-archive", source_path),
        ("native-linux-x86_64-archive", linux_path),
        ("native-windows-x64-archive", windows_path),
    ):
        package.verify_archive(path)
        assert generated[artifact_id]
    if cacheable:
        PAYLOAD_BYTES_CACHE = generated
    return generated


def make_inputs(tmp_path: Path) -> tuple[Path, Path, Path, tuple[str, ...]]:
    value = manifest()
    version = value["release_candidate"]
    payload_dir = tmp_path / "payload"
    source_dir = tmp_path / "sources"
    reports_dir = tmp_path / "reports"
    release_dir = tmp_path / "release"
    payload_dir.mkdir()
    source_dir.mkdir()
    reports_dir.mkdir()
    release_dir.mkdir()
    payload_bytes = valid_payload_bytes(tmp_path, value)
    for artifact_id, relative in payload_names(version).items():
        (payload_dir / relative).write_bytes(payload_bytes[artifact_id])
    evidence_ids = tuple(
        row["id"] for row in value["evidence"] if row.get("release_gate") is True
    )
    collector = load_module("release_evidence_collect_for_assembly", COLLECT)
    for spec in collector.SOURCE_SPECS:
        if spec.source_id == "supply-chain":
            continue
        source = {
            "schema": spec.schema,
            "source_id": spec.source_id,
            "evidence_id": spec.evidence_id,
            "status": "passed",
            "release_evidence": True,
            "fail_closed": False,
            "version": version,
            "platform": spec.platform,
            "contract": value["contracts"],
            "checks": {check: True for check in spec.checks},
            "details": {
                "producer_id": collector.EXPECTED_PRODUCER_IDS[spec.source_id],
                "phase": "pre-sign",
                "outcomes": {
                    check: {
                        "status": "passed",
                        "observations": [{"kind": "fixture", "value": check}],
                    }
                    for check in spec.checks
                },
                "artifacts": {},
            },
        }
        payload_bindings = {
            ("native-linux", "linux-release-profile"): "native-linux-x86_64-archive",
            ("native-windows", "windows-release-profile"): "native-windows-x64-archive",
            ("oci-linux", "oci-release-profile"): "oci-linux-amd64-image",
            ("failure-soak", "linux-release-profile"): "native-linux-x86_64-archive",
            (
                "reference-performance",
                "linux-release-profile",
            ): "native-linux-x86_64-archive",
        }
        for binding in collector.REQUIRED_ARTIFACT_BINDINGS[spec.source_id]:
            payload_binding = payload_bindings.get((spec.source_id, binding))
            if payload_binding is None:
                source["details"]["artifacts"][binding] = {
                    "sha256": "sha256:" + "a" * 64,
                    "size_bytes": 1,
                }
            else:
                payload_path = payload_dir / payload_names(version)[payload_binding]
                source["details"]["artifacts"][binding] = {
                    "sha256": "sha256:"
                    + hashlib.sha256(payload_path.read_bytes()).hexdigest(),
                    "size_bytes": payload_path.stat().st_size,
                }
        (source_dir / f"{spec.evidence_id}.json").write_text(
            json.dumps(source, sort_keys=True) + "\n", encoding="utf-8"
        )
    fake_bin = tmp_path / "fake-bin"
    fake_bin.mkdir()
    fake_cosign = fake_bin / "cosign"
    fake_cosign.write_text(
        "#!/bin/sh\n"
        'if [ "$1" = sign-blob ]; then\n'
        '  while [ "$#" -gt 0 ]; do\n'
        '    if [ "$1" = --bundle ]; then shift; bundle="$1"; fi\n'
        "    shift\n"
        "  done\n"
        "  printf '{}\\n' > \"$bundle\"\n"
        "  exit 0\n"
        "fi\n"
        "exit 0\n",
        encoding="utf-8",
    )
    fake_cosign.chmod(0o755)
    source_args = [
        f"--source={spec.source_id}={source_dir / f'{spec.evidence_id}.json'}"
        for spec in collector.SOURCE_SPECS
        if spec.source_id != "supply-chain"
    ]
    environment = os.environ.copy()
    environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
    environment["COSIGN_CERTIFICATE_IDENTITY"] = "https://example.test/workflow"
    environment["COSIGN_CERTIFICATE_OIDC_ISSUER"] = (
        "https://token.actions.githubusercontent.com"
    )
    environment.update(
        {
            "GITHUB_ACTIONS": "true",
            "GITHUB_REPOSITORY": "imom39a/worldstream",
            "GITHUB_SERVER_URL": "https://github.com",
            "GITHUB_WORKFLOW_REF": (
                "imom39a/worldstream/.github/workflows/compatibility-gates.yml"
                "@refs/heads/main"
            ),
            "GITHUB_RUN_ID": "123456789",
            "GITHUB_RUN_ATTEMPT": "1",
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REF": "refs/heads/main",
            "WORLDSTREAM_RELEASE_INPUT": "true",
            "RUNNER_OS": "Linux",
            "RUNNER_ARCH": "X64",
            "ImageOS": "ubuntu24",
            "ImageVersion": "20260817.1.0",
        }
    )
    supply = subprocess.run(
        [
            sys.executable,
            str(SUPPLY_CHAIN),
            "--release-dir",
            str(release_dir),
            "--payload-dir",
            str(payload_dir),
            "--producer-output",
            str(tmp_path / "supply-chain-producer.json"),
            "--source-output",
            str(source_dir / "checksums-signature-sbom-provenance.json"),
            *source_args,
        ],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    assert supply.returncode == 0, supply.stderr
    sources = {
        spec.source_id: source_dir / f"{spec.evidence_id}.json"
        for spec in collector.SOURCE_SPECS
    }
    collector.collect(
        reports_dir, sources, ROOT / "compatibility.toml", ROOT / "compatibility.json"
    )
    return payload_dir, reports_dir, release_dir, evidence_ids


def run_script(
    script: Path, release: Path, payload: Path, reports: Path
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SOURCE_DATE_EPOCH"] = "0"
    return subprocess.run(
        [
            sys.executable,
            str(script),
            "--release-dir",
            str(release),
            "--payload-dir",
            str(payload),
            "--reports-dir",
            str(reports),
        ],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )


def assembled_release(tmp_path: Path) -> tuple[Path, Path, Path, tuple[str, ...]]:
    payload, reports, release, evidence_ids = make_inputs(tmp_path)
    assembled = run_script(ASSEMBLE, release, payload, reports)
    assert assembled.returncode == 0, assembled.stderr
    (release / "sigstore.bundle.json").write_text(
        json.dumps(
            {
                "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.3",
                "verificationMaterial": {"tlogEntries": [{}]},
                "messageSignature": {
                    "messageDigest": {"algorithm": "SHA2_256", "digest": "encoded"}
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    return payload, reports, release, evidence_ids


def verify_supply_chain(release: Path) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    fake_bin = release.parent / "fake-bin"
    environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
    environment["COSIGN_CERTIFICATE_IDENTITY"] = "https://example.test/workflow"
    environment["COSIGN_CERTIFICATE_OIDC_ISSUER"] = (
        "https://token.actions.githubusercontent.com"
    )
    return subprocess.run(
        [
            sys.executable,
            str(SUPPLY_CHAIN),
            "--verify",
            "--release-dir",
            str(release),
        ],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )


def test_unsigned_aggregation_and_finalization_reject_oidc_capability(
    tmp_path, monkeypatch
):
    supply = load_module("release_supply_chain_oidc_boundary", SUPPLY_CHAIN)
    monkeypatch.setenv("ACTIONS_ID_TOKEN_REQUEST_URL", "https://example.invalid/oidc")
    monkeypatch.setenv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "fixture-token")

    with pytest.raises(supply.ASSEMBLER.AssemblyError, match="without.*OIDC"):
        supply.prepare_unsigned(
            tmp_path / "release",
            tmp_path / "payload",
            {},
            ROOT / "compatibility.toml",
            ROOT / "compatibility.json",
        )
    with pytest.raises(supply.ASSEMBLER.AssemblyError, match="without.*OIDC"):
        supply.finalize_signed(
            tmp_path / "release",
            tmp_path / "producer.json",
            tmp_path / "source.json",
            ROOT / "compatibility.toml",
            ROOT / "compatibility.json",
        )


def test_assembly_parser_rejects_duplicate_signed_security_keys(tmp_path):
    assembler = load_module("strict_release_assembly", ASSEMBLE)
    document = tmp_path / "provenance.json"
    document.write_bytes(
        b'{"predicate":{"buildDefinition":{},"buildDefinition":{"hidden":true}}}'
    )

    with pytest.raises(
        assembler.AssemblyError, match="duplicate key 'buildDefinition'"
    ):
        assembler.json_object(document, "signed SLSA provenance")


def test_assembly_bounded_json_reader_and_streamed_copy(tmp_path, monkeypatch):
    assembler = load_module("bounded_release_assembly", ASSEMBLE)
    collector = assembler.evidence_collector()
    document = b'{"value":"bounded"}'
    monkeypatch.setattr(
        collector.BUILD_IDENTITY, "MAX_RELEASE_JSON_BYTES", len(document)
    )
    exact = tmp_path / "exact.json"
    exact.write_bytes(document)

    assert assembler.json_object(exact, "exact JSON") == {"value": "bounded"}
    exact.write_bytes(document + b" ")
    with pytest.raises(assembler.AssemblyError, match="too large"):
        assembler.json_object(exact, "oversized JSON")

    source = tmp_path / "payload.bin"
    destination = tmp_path / "release" / "payload.bin"
    payload = (b"stream-copy" * 257) + b"\n"
    source.write_bytes(payload)
    digest, size = assembler.copy_atomic(
        source,
        destination,
        "payload fixture",
        maximum=len(payload),
    )
    assert digest == hashlib.sha256(payload).hexdigest()
    assert size == len(payload)
    assert destination.read_bytes() == payload

    destination.write_bytes(b"preserved")
    with pytest.raises(assembler.AssemblyError, match="exceeds"):
        assembler.copy_atomic(
            source,
            destination,
            "oversized payload fixture",
            maximum=len(payload) - 1,
        )
    assert destination.read_bytes() == b"preserved"


def test_assembly_generates_exact_17_subjects_and_no_sigstore_digest(tmp_path):
    identity = load_module(
        "release_build_identity_assembly_assertions",
        ROOT / "scripts/release_build_identity.py",
    )
    _payload, _reports, release, evidence_ids = assembled_release(tmp_path)
    metadata = json.loads((release / "release-manifest.json").read_text())
    assert len(evidence_ids) == 14
    assert set(metadata["artifact_digests"]) == {
        "source-archive",
        "native-linux-x86_64-archive",
        "native-windows-x64-archive",
        "oci-linux-amd64-image",
        "checksums",
        "spdx-sbom",
        "slsa-provenance",
    }
    assert "sigstore-bundle" not in metadata["artifact_digests"]
    assert metadata["verification_material"] == {
        "sigstore-bundle": {"path": "sigstore.bundle.json"}
    }
    spdx = json.loads((release / "sbom.spdx.json").read_text())
    assert len(spdx["files"]) == 17
    assert spdx["dataLicense"] == "CC0-1.0"
    assert spdx["creationInfo"]["creators"] == [
        "Tool: worldstream-release-supply-chain-1.0"
    ]
    assert all(
        {checksum["algorithm"] for checksum in item["checksums"]} == {"SHA1", "SHA256"}
        for item in spdx["files"]
    )
    package_names = {package["name"] for package in spdx["packages"]}
    for profile in ("linux-x86_64", "windows-x64", "oci-linux-amd64"):
        assert f"{profile}-bundled-sqlite-c-compiler" in package_names
        assert f"{profile}-bundled-sqlite-archiver" in package_names
        assert f"{profile}-final-linker" in package_names
    zig_package = next(
        package for package in spdx["packages"] if package["name"] == "zig"
    )
    assert zig_package["downloadLocation"] == identity.ZIG_LINUX_X86_64_URL
    assert zig_package["checksums"] == [
        {
            "algorithm": "SHA256",
            "checksumValue": identity.ZIG_LINUX_X86_64_SHA256,
        }
    ]
    provenance = json.loads((release / "provenance.json").read_text())
    assert len(provenance["subject"]) == 17
    aggregation = json.loads(
        base64.b64decode(
            provenance["predicate"]["runDetails"]["byproducts"][1]["content"]
        )
    )
    payload_rows = aggregation["payload_producers"]
    assert len(payload_rows) == 4
    assert all(
        row["observed_build_environment"]["runner"]["provider"] == "github-actions"
        for row in payload_rows
    )
    source_row = next(
        row for row in payload_rows if row["artifact_id"] == "source-archive"
    )
    assert source_row["observed_build_environment"]["bundled_sqlite"] is None
    assert source_row["observed_build_environment"]["final_linker"] is None
    assert all(
        row["observed_build_environment"]["bundled_sqlite"] is not None
        for row in payload_rows
        if row["artifact_id"] != "source-archive"
    )
    assert all(
        row["observed_build_environment"]["final_linker"] is not None
        for row in payload_rows
        if row["artifact_id"] != "source-archive"
    )
    resolved_dependencies = provenance["predicate"]["buildDefinition"][
        "resolvedDependencies"
    ]
    assert {
        "uri": identity.ZIG_LINUX_X86_64_URL,
        "digest": {"sha256": identity.ZIG_LINUX_X86_64_SHA256},
    } in resolved_dependencies
    oci_dependencies = {
        item["uri"]: item["digest"]
        for item in resolved_dependencies
        if item["uri"].startswith("oci://")
    }
    assert set(oci_dependencies) == {
        "oci://docker.io/library/alpine",
        "oci://docker.io/moby/buildkit",
        "oci://docker.io/docker/dockerfile:1.7",
    }
    assert all(set(digest) == {"sha256"} for digest in oci_dependencies.values())
    assert len((release / "SHA256SUMS").read_text().splitlines()) == 17


def test_spdx_namespace_is_unique_for_each_exact_document_version(tmp_path):
    supply_chain = load_module("release_supply_chain_namespace", SUPPLY_CHAIN)
    subject = tmp_path / "subject.bin"
    subject.write_bytes(b"first subject version\n")
    subjects = {"payload/subject.bin": subject}
    first = supply_chain.spdx_document_namespace(
        "0.1.0", "a" * 64, subjects, "2026-08-22T00:00:00Z"
    )

    assert first == supply_chain.spdx_document_namespace(
        "0.1.0", "a" * 64, subjects, "2026-08-22T00:00:00Z"
    )
    assert first != supply_chain.spdx_document_namespace(
        "0.1.0", "a" * 64, subjects, "2026-08-22T00:00:01Z"
    )
    subject.write_bytes(b"second subject version\n")
    assert first != supply_chain.spdx_document_namespace(
        "0.1.0", "a" * 64, subjects, "2026-08-22T00:00:00Z"
    )
    assert first.startswith("https://github.com/imom39a/worldstream/spdx/0.1.0/")
    assert "#" not in first


@pytest.mark.parametrize("tamper", ["namespace", "file_id"])
def test_spdx_verifier_recomputes_document_and_element_identity(tmp_path, tamper):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    path = release / "sbom.spdx.json"
    value = json.loads(path.read_text(encoding="utf-8"))
    if tamper == "namespace":
        value["documentNamespace"] = "https://example.invalid/fabricated"
    else:
        value["files"][0]["SPDXID"] = "SPDXRef-ReleaseSubject-fabricated"
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    inventory = json.loads(
        (release / "supply-chain/subject-inventory.json").read_text(encoding="utf-8")
    )
    subjects = {item["path"]: release / item["path"] for item in inventory["subjects"]}
    assembler = load_module(f"release_assembly_spdx_identity_{tamper}", ASSEMBLE)
    with pytest.raises(assembler.AssemblyError, match="SPDX"):
        assembler.validate_spdx_subjects(
            path,
            subjects,
            version=manifest()["release_candidate"],
            manifest_sha256=hashlib.sha256(
                (ROOT / "compatibility.json").read_bytes()
            ).hexdigest(),
        )


def test_spdx_verifier_rejects_impossible_utc_timestamp(tmp_path):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    path = release / "sbom.spdx.json"
    value = json.loads(path.read_text(encoding="utf-8"))
    value["creationInfo"]["created"] = "2026-99-99T99:99:99Z"
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    inventory = json.loads(
        (release / "supply-chain/subject-inventory.json").read_text(encoding="utf-8")
    )
    subjects = {item["path"]: release / item["path"] for item in inventory["subjects"]}
    assembler = load_module("release_assembly_spdx_timestamp", ASSEMBLE)
    with pytest.raises(assembler.AssemblyError, match="creation time"):
        assembler.validate_spdx_subjects(
            path,
            subjects,
            version=manifest()["release_candidate"],
            manifest_sha256=hashlib.sha256(
                (ROOT / "compatibility.json").read_bytes()
            ).hexdigest(),
        )


def test_two_level_supply_chain_binds_inventory_and_verifies_both_signatures(tmp_path):
    _payload, reports, release, _evidence_ids = assembled_release(tmp_path)
    inventory = json.loads(
        (release / "supply-chain/subject-inventory.json").read_text()
    )
    assert inventory["schema"] == "worldstream/release-subject-inventory/v1"
    assert inventory["phase"] == "pre-sign"
    assert len(inventory["subjects"]) == 17
    supply = json.loads(
        (reports / "checksums-signature-sbom-provenance.json").read_text()
    )
    assert set(supply["producer_details"]["artifacts"]) == {
        "subject-inventory",
        "subject-signature",
        "checksums",
        "spdx-sbom",
        "slsa-provenance",
    }
    for check in ("checksums", "spdx_subjects", "slsa_subjects"):
        assert (
            "17 signed subjects"
            in supply["producer_details"]["outcomes"][check]["observations"][0]["value"]
        )
    assert (
        "identity=https://example.test/workflow"
        in supply["producer_details"]["outcomes"]["sigstore_identity"]["observations"][
            0
        ]["value"]
    )
    verified = verify_supply_chain(release)
    assert verified.returncode == 0, verified.stderr


def test_structural_only_wrapper_returns_11_without_invoking_cosign(tmp_path):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    wrapper_bin = tmp_path / "wrapper-bin"
    wrapper_bin.mkdir()
    (wrapper_bin / "python3").symlink_to(sys.executable)
    marker = tmp_path / "cosign-invoked"
    cosign = wrapper_bin / "cosign"
    cosign.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$*" >> "$COSIGN_MARKER"\nexit 99\n',
        encoding="utf-8",
    )
    cosign.chmod(0o755)
    environment = os.environ.copy()
    environment["PATH"] = f"{wrapper_bin}:/usr/bin:/bin"
    environment["COSIGN_MARKER"] = str(marker)
    environment["COSIGN_CERTIFICATE_IDENTITY"] = "https://example.test/workflow"
    environment["COSIGN_CERTIFICATE_OIDC_ISSUER"] = (
        "https://token.actions.githubusercontent.com"
    )

    result = subprocess.run(
        [str(VERIFY_RELEASE_SH), str(release), "--structural-only"],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 11, result.stderr
    assert not marker.exists()


def test_package_and_gate_verifiers_require_both_signature_levels(
    tmp_path, monkeypatch
):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    fake_bin = tmp_path / "fake-bin"
    marker = tmp_path / "cosign-subjects"
    cosign = fake_bin / "cosign"
    cosign.write_text(
        "#!/bin/sh\n"
        "last=''\n"
        'for argument in "$@"; do last="$argument"; done\n'
        'printf "%s\\n" "$last" >> "$COSIGN_MARKER"\n'
        'if [ "${COSIGN_REJECT_PRE_SIGN:-0}" = 1 ] && '
        '   [ "$(basename "$last")" = subject-inventory.json ]; then\n'
        "  exit 42\n"
        "fi\n"
        "exit 0\n",
        encoding="utf-8",
    )
    cosign.chmod(0o755)
    environment = os.environ.copy()
    environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
    environment["COSIGN_MARKER"] = str(marker)
    environment["COSIGN_CERTIFICATE_IDENTITY"] = "https://example.test/workflow"
    environment["COSIGN_CERTIFICATE_OIDC_ISSUER"] = (
        "https://token.actions.githubusercontent.com"
    )

    verified = subprocess.run(
        [sys.executable, str(PACKAGE), "verify", str(release)],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    assert verified.returncode == 0, verified.stderr
    assert [Path(line).name for line in marker.read_text().splitlines()] == [
        "subject-inventory.json",
        "release-manifest.json",
    ]

    marker.unlink()
    monkeypatch.setenv("PATH", environment["PATH"])
    monkeypatch.setenv("COSIGN_MARKER", str(marker))
    monkeypatch.setenv(
        "COSIGN_CERTIFICATE_IDENTITY", environment["COSIGN_CERTIFICATE_IDENTITY"]
    )
    monkeypatch.setenv(
        "COSIGN_CERTIFICATE_OIDC_ISSUER",
        environment["COSIGN_CERTIFICATE_OIDC_ISSUER"],
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release))
    gates = load_module("release_gate_dual_signature", ROOT / "scripts/gates.py")
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    gates.verify_release_artifacts(runner, gates.load_manifest())
    assert any(
        outcome.name == "release-signature-verifier" and outcome.status == "PASS"
        for outcome in runner.outcomes
    )
    assert [Path(line).name for line in marker.read_text().splitlines()] == [
        "subject-inventory.json",
        "release-manifest.json",
    ]

    marker.unlink()
    environment["COSIGN_REJECT_PRE_SIGN"] = "1"
    rejected = subprocess.run(
        [sys.executable, str(PACKAGE), "verify", str(release)],
        cwd=ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    assert rejected.returncode != 0
    assert "pre-sign subject inventory signature verification failed" in rejected.stderr


def test_powershell_wrapper_routes_deep_and_report_verification_through_package():
    script = VERIFY_RELEASE_PS1.read_text(encoding="utf-8")

    assert "@('verify') + [string[]]$VerifyArgs" in script
    assert "@('report', $Artifact, '--check', $ReportPath)" in script
    assert "$PackageExitCode -eq 11" in script
    assert "WORLDSTREAM_RELEASE_PYTHON" in script
    assert ".python-version" in script
    assert "& $Python -I" in script
    assert "Get-Command py" not in script
    assert "Get-FileHash" not in script


@pytest.mark.parametrize(
    "tamper", ["failed_status", "source_citation", "passed_substitution"]
)
def test_every_final_verifier_rejects_resigned_invalid_normalized_evidence(
    tmp_path, monkeypatch, tamper
):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    evidence_id = "manifest-syntax-parity"
    report_path = release / f"evidence/{evidence_id}.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    if tamper == "failed_status":
        report["status"] = "failed"
        report["release_evidence"] = False
        report["fail_closed"] = True
        report["summary"]["failures"] = 1
    elif tamper == "source_citation":
        report["source_report"]["sha256"] = "sha256:" + "0" * 64
    else:
        report["platform"] = "fabricated-platform"
        report["checks"] = {"fabricated-check": True}
        report["producer_details"]["producer_id"] = "fabricated-producer/v1"
        report["producer_details"]["outcomes"] = {
            "fabricated-check": {
                "status": "passed",
                "observations": [{"kind": "claim", "value": "fabricated"}],
            }
        }
        report["producer_details"]["artifacts"] = {
            "fabricated-artifact": {
                "sha256": "sha256:" + "1" * 64,
                "size_bytes": 1,
            }
        }
        report["source_report"]["source_id"] = "fabricated-source"
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n", encoding="utf-8")
    metadata_path = release / "release-manifest.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    metadata["evidence_digests"][evidence_id] = (
        "sha256:" + hashlib.sha256(report_path.read_bytes()).hexdigest()
    )
    metadata_path.write_text(
        json.dumps(metadata, sort_keys=True) + "\n", encoding="utf-8"
    )

    verified = verify_supply_chain(release)
    assert verified.returncode != 0
    assert "evidence" in verified.stderr.lower()

    package = load_module(
        f"release_package_semantics_{tamper}", ROOT / "scripts/package.py"
    )
    with pytest.raises(package.PackageError, match="release evidence verification"):
        package.verify_release_directory(release, structural_only=True)

    gates = load_module(f"release_gate_semantics_{tamper}", ROOT / "scripts/gates.py")
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    gates.verify_release_artifacts(runner, gates.load_manifest())
    assert any(
        outcome.name == "release-evidence-semantics" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )


def test_final_verifier_rejects_tampered_signed_subject_inventory(tmp_path):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    inventory_path = release / "supply-chain/subject-inventory.json"
    inventory = json.loads(inventory_path.read_text())
    inventory["subjects"][0]["sha256"] = "sha256:" + "0" * 64
    inventory_path.write_text(json.dumps(inventory) + "\n")

    verified = verify_supply_chain(release)

    assert verified.returncode != 0
    assert "pre-sign subject bytes changed" in verified.stderr


def test_assembly_reports_missing_evidence_precisely(tmp_path):
    payload, reports, release, evidence_ids = make_inputs(tmp_path)
    missing = evidence_ids[-1]
    (reports / f"{missing}.json").unlink()
    result = run_script(ASSEMBLE, release, payload, reports)
    assert result.returncode != 0
    assert f"missing evidence reports: {missing}" in result.stderr


@pytest.mark.parametrize(
    "artifact_id",
    [
        "source-archive",
        "native-linux-x86_64-archive",
        "native-windows-x64-archive",
        "oci-linux-amd64-image",
    ],
)
def test_assembly_deep_verifies_every_payload_before_signing(tmp_path, artifact_id):
    payload, reports, release, _evidence_ids = make_inputs(tmp_path)
    version = manifest()["release_candidate"]
    (payload / payload_names(version)[artifact_id]).write_bytes(
        f"not a valid {artifact_id}\n".encode()
    )

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert f"release payload {artifact_id} failed deep verification" in result.stderr


@pytest.mark.parametrize(
    ("evidence_id", "binding_id"),
    [
        ("native-linux-release-profile", "linux-release-profile"),
        ("native-windows-release-profile", "windows-release-profile"),
        ("oci-linux-amd64-release-profile", "oci-release-profile"),
        (
            "failure-fuzz-resource-and-one-hour-sqlite-soak",
            "linux-release-profile",
        ),
        ("reference-performance-per-backend", "linux-release-profile"),
    ],
)
def test_assembly_requires_platform_binding_to_exact_payload_bytes(
    tmp_path, evidence_id, binding_id
):
    payload, reports, release, _evidence_ids = make_inputs(tmp_path)
    report_path = reports / f"{evidence_id}.json"
    report = json.loads(report_path.read_text())
    report["producer_details"]["artifacts"][binding_id]["sha256"] = "sha256:" + "0" * 64
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n")

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert "does not bind the exact shipped payload" in result.stderr


def test_assembly_rejects_self_consistent_archive_with_a_different_contract(tmp_path):
    payload, reports, release, _evidence_ids = make_inputs(tmp_path)
    helpers = load_module("release_assembly_drift_package_helpers", PACKAGE_SMOKE)
    package = helpers.PACKAGE
    authored = (
        (ROOT / "compatibility.toml")
        .read_text(encoding="utf-8")
        .replace('wire = "0.1"', 'wire = "0.2"', 1)
    )
    drifted_manifest = tomllib.loads(authored)
    drifted_json = (
        json.dumps(drifted_manifest, indent=2, sort_keys=True) + "\n"
    ).encode()
    fixture_root = tmp_path / "drifted-native"
    inputs = helpers.fixture_inputs(fixture_root)
    helpers.add_fixture_client_identities(fixture_root, inputs, drifted_manifest)
    shutil.copy2(
        ROOT / "examples/heist/parity_fixture.json",
        fixture_root / "examples/heist/parity_fixture.json",
    )
    version = drifted_manifest["release_candidate"]
    files = package.collect_package_files(
        package.TARGETS["linux-x86_64"],
        version,
        authored.encode(),
        drifted_json,
        inputs,
        0,
        require_clean_checkout=False,
    )
    path = payload / payload_names(version)["native-linux-x86_64-archive"]
    package.write_tar_gz(path, f"worldstream-{version}-linux-x86_64", files, 0)
    report_path = reports / "native-linux-release-profile.json"
    report = json.loads(report_path.read_text())
    report["producer_details"]["artifacts"]["linux-release-profile"] = {
        "sha256": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
        "size_bytes": path.stat().st_size,
    }
    report_path.write_text(json.dumps(report, sort_keys=True) + "\n")

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert "embedded release contract drifted" in result.stderr


def test_payload_verifier_requires_the_exact_authoritative_manifest(tmp_path):
    payload, reports, _release, _evidence_ids = make_inputs(tmp_path)
    authored = (
        (ROOT / "compatibility.toml")
        .read_text(encoding="utf-8")
        .replace('python_sdk_min = "3.11"', 'python_sdk_min = "3.12"', 1)
    )
    assert authored != (ROOT / "compatibility.toml").read_text(encoding="utf-8")
    drifted_manifest = tomllib.loads(authored)
    drifted_payloads = valid_payload_bytes(
        tmp_path / "all-payload-manifest-drift",
        drifted_manifest,
        authored_manifest=authored.encode(),
    )
    version = drifted_manifest["release_candidate"]
    paths = {
        artifact_id: payload / relative
        for artifact_id, relative in payload_names(version).items()
    }
    for artifact_id, path in paths.items():
        path.write_bytes(drifted_payloads[artifact_id])

    bindings = {
        "native-linux-release-profile": (
            "native-linux-x86_64-archive",
            "linux-release-profile",
        ),
        "native-windows-release-profile": (
            "native-windows-x64-archive",
            "windows-release-profile",
        ),
        "oci-linux-amd64-release-profile": (
            "oci-linux-amd64-image",
            "oci-release-profile",
        ),
        "failure-fuzz-resource-and-one-hour-sqlite-soak": (
            "native-linux-x86_64-archive",
            "linux-release-profile",
        ),
        "reference-performance-per-backend": (
            "native-linux-x86_64-archive",
            "linux-release-profile",
        ),
    }
    report_paths = {
        evidence_id: reports / f"{evidence_id}.json" for evidence_id in bindings
    }
    for evidence_id, (artifact_id, binding_id) in bindings.items():
        report_path = report_paths[evidence_id]
        report = json.loads(report_path.read_text(encoding="utf-8"))
        report["contract"] = drifted_manifest["contracts"]
        artifact = paths[artifact_id]
        report["producer_details"]["artifacts"][binding_id] = {
            "sha256": "sha256:" + hashlib.sha256(artifact.read_bytes()).hexdigest(),
            "size_bytes": artifact.stat().st_size,
        }
        report_path.write_text(
            json.dumps(report, sort_keys=True) + "\n", encoding="utf-8"
        )

    assembler = load_module("release_assembly_exact_manifest", ASSEMBLE)
    authoritative_digest = hashlib.sha256(
        (ROOT / "compatibility.json").read_bytes()
    ).hexdigest()
    with pytest.raises(
        assembler.AssemblyError,
        match="exact authoritative manifest",
    ):
        assembler.validate_release_payloads(paths, report_paths, authoritative_digest)


def test_assembly_rejects_failed_report(tmp_path):
    payload, reports, release, evidence_ids = make_inputs(tmp_path)
    failed = evidence_ids[0]
    report = json.loads((reports / f"{failed}.json").read_text())
    report["status"] = "incomplete"
    (reports / f"{failed}.json").write_text(json.dumps(report) + "\n")
    result = run_script(ASSEMBLE, release, payload, reports)
    assert result.returncode != 0
    assert f"evidence report {failed} is not passed" in result.stderr


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("release_evidence", None, "must set release_evidence=true"),
        ("fail_closed", None, "must set fail_closed=false"),
        ("schema", "worldstream/diagnostic/v1", "wrong normalized schema"),
        ("version", "0.1.0-tampered", "wrong release version"),
    ],
)
def test_assembly_requires_explicit_normalized_release_identity(
    tmp_path, field, value, message
):
    payload, reports, release, evidence_ids = make_inputs(tmp_path)
    target = evidence_ids[0]
    report_path = reports / f"{target}.json"
    report = json.loads(report_path.read_text())
    report[field] = value
    report_path.write_text(json.dumps(report) + "\n")

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert message in result.stderr


@pytest.mark.parametrize("missing", ["release_evidence", "fail_closed"])
def test_assembly_rejects_omitted_truth_labels(tmp_path, missing):
    payload, reports, release, evidence_ids = make_inputs(tmp_path)
    target = evidence_ids[0]
    report_path = reports / f"{target}.json"
    report = json.loads(report_path.read_text())
    report.pop(missing)
    report_path.write_text(json.dumps(report) + "\n")

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert "not a normalized collector report" in result.stderr
    assert missing in result.stderr


def test_assembly_rejects_extra_and_symlink_reports(tmp_path):
    payload, reports, release, _evidence_ids = make_inputs(tmp_path)
    (reports / "unexpected.json").write_text("{}\n")
    result = run_script(ASSEMBLE, release, payload, reports)
    assert result.returncode != 0
    assert "extra evidence reports: unexpected.json" in result.stderr

    (reports / "unexpected.json").unlink()
    os.symlink(reports / "manifest-syntax-parity.json", reports / "linked.json")
    result = run_script(ASSEMBLE, release, payload, reports)
    assert result.returncode != 0
    assert "evidence reports directory contains a symlink" in result.stderr


def test_package_verifier_rejects_self_reference_and_altered_subject(tmp_path):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    package = load_module("release_package_smoke", ROOT / "scripts/package.py")
    metadata_path = release / "release-manifest.json"
    metadata = json.loads(metadata_path.read_text())
    metadata["artifact_digests"]["sigstore-bundle"] = "sha256:" + "a" * 64
    metadata_path.write_text(json.dumps(metadata) + "\n")
    with pytest.raises(package.PackageError, match="Sigstore"):
        package.verify_release_directory(release, structural_only=True)

    metadata = json.loads(metadata_path.read_text())
    metadata["artifact_digests"].pop("sigstore-bundle")
    metadata_path.write_text(json.dumps(metadata) + "\n")
    provenance_path = release / "provenance.json"
    provenance = json.loads(provenance_path.read_text())
    provenance["subject"][0]["digest"]["sha256"] = "0" * 64
    provenance_path.write_text(json.dumps(provenance) + "\n")
    metadata["artifact_digests"]["slsa-provenance"] = "sha256:" + package.sha256_file(
        provenance_path
    )
    supply_id = "checksums-signature-sbom-provenance"
    supply_path = release / f"evidence/{supply_id}.json"
    supply = json.loads(supply_path.read_text())
    supply["producer_details"]["artifacts"]["slsa-provenance"] = {
        "sha256": "sha256:" + package.sha256_file(provenance_path),
        "size_bytes": provenance_path.stat().st_size,
    }
    supply_path.write_text(json.dumps(supply, sort_keys=True) + "\n")
    metadata["evidence_digests"][supply_id] = "sha256:" + package.sha256_file(
        supply_path
    )
    metadata_path.write_text(json.dumps(metadata) + "\n")
    with pytest.raises(package.PackageError, match="provenance subject .*mismatch"):
        package.verify_release_directory(release, structural_only=True)


@pytest.mark.parametrize(
    "artifact_id",
    [
        "source-archive",
        "native-linux-x86_64-archive",
        "native-windows-x64-archive",
        "oci-linux-amd64-image",
    ],
)
def test_final_package_verifier_deep_checks_redigested_payload(tmp_path, artifact_id):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    package = load_module(
        f"release_package_deep_{artifact_id}", ROOT / "scripts/package.py"
    )
    metadata_path = release / "release-manifest.json"
    metadata = json.loads(metadata_path.read_text())
    artifact_path = release / metadata["artifacts"][artifact_id]
    artifact_path.write_bytes(f"redigested fake {artifact_id}\n".encode())
    metadata["artifact_digests"][artifact_id] = "sha256:" + package.sha256_file(
        artifact_path
    )
    metadata_path.write_text(json.dumps(metadata, sort_keys=True) + "\n")

    with pytest.raises(package.PackageError, match="failed deep verification"):
        package.verify_release_directory(release, structural_only=True)


def test_gate_verifier_rejects_sigstore_digest_and_accepts_path_only(
    tmp_path, monkeypatch
):
    _payload, _reports, release, _evidence_ids = assembled_release(tmp_path)
    gates = load_module("release_gates_smoke", ROOT / "scripts/gates.py")
    metadata = json.loads((release / "release-manifest.json").read_text())
    value = gates.load_manifest()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, value) is not None

    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    gates.verify_release_artifacts(runner, value)
    assert not any(
        outcome.name == "release-artifact-manifest" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )
    assert any(
        outcome.name == "release-artifact-sigstore-bundle" and outcome.status == "PASS"
        for outcome in runner.outcomes
    )

    metadata["artifact_digests"]["sigstore-bundle"] = "sha256:" + "b" * 64
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, value) is None
    assert any("path-only" in outcome.detail for outcome in runner.outcomes)


def test_supply_chain_rejects_post_sign_self_reference(tmp_path):
    payload, reports, release, _evidence_ids = assembled_release(tmp_path)
    report_path = reports / "checksums-signature-sbom-provenance.json"
    report = json.loads(report_path.read_text())
    report["producer_details"]["final_subject_digests"] = {
        "release-manifest.json": "sha256:" + "b" * 64
    }
    report_path.write_text(json.dumps(report) + "\n")

    result = run_script(ASSEMBLE, release, payload, reports)

    assert result.returncode != 0
    assert "not a pre-sign producer report" in result.stderr
