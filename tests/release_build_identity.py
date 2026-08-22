"""Focused fail-closed tests for release source, toolchain, and component identity."""

from __future__ import annotations

import base64
import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release_build_identity.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_build_identity", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def source_entries(module) -> dict[str, bytes]:
    return module.source_entries_from_root(ROOT)


def revision() -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def git(tmp_path: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *arguments],
        cwd=tmp_path,
        check=True,
        capture_output=True,
        text=True,
    )


def committed_repository(tmp_path: Path) -> str:
    git(tmp_path, "init", "--quiet")
    git(tmp_path, "config", "user.name", "WorldStream Test")
    git(tmp_path, "config", "user.email", "worldstream@example.invalid")
    (tmp_path / "tracked.txt").write_text("committed\n", encoding="utf-8")
    git(tmp_path, "add", "tracked.txt")
    git(tmp_path, "commit", "--quiet", "-m", "fixture")
    return git(tmp_path, "rev-parse", "HEAD").stdout.strip()


def source_identity(module, entries: dict[str, bytes]):
    return module.build_identity(
        target="source",
        revision=revision(),
        source_entries=entries,
        source_date_epoch=0,
        manifest_sha256=hashlib.sha256(entries["compatibility.json"]).hexdigest(),
    )


def hosted_build_environment(module, target: str) -> dict:
    runner = {
        "provider": "github-actions",
        **module.HOSTED_RUNNER_FACTS[target],
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
    root = (
        "C:\\hostedtoolcache\\worldstream"
        if windows
        else "/opt/hostedtoolcache/worldstream"
    )
    reported = (
        {
            "rustc": "x86_64-pc-windows-msvc",
            "archiver": "x64-coff-library",
            "c_compiler": "x64",
            "linker": "x64",
        }
        if windows
        else {
            "rustc": "x86_64-unknown-linux-gnu",
            "archiver": "gnu-archive",
            "c_compiler": "x86_64-unknown-linux-musl",
            "linker": "elf_x86_64",
        }
    )
    suffix = ".exe" if windows else ""
    return {
        "runner": runner,
        "rustc": {
            "path": f"{root}/rustc{suffix}",
            "version": "rustc 1.97.1 (fixture)",
            "target": module.TARGET_TRIPLES[target],
            "reported_target": reported["rustc"],
        },
        "bundled_sqlite": {
            "archiver": {
                "path": f"{root}/Hostx64/x64/lib.exe"
                if windows
                else f"{root}/zig-musl-ar",
                "version": "fixture archiver 1.0",
                "target": module.TARGET_TRIPLES[target],
                "reported_target": reported["archiver"],
            },
            "c_compiler": {
                "path": f"{root}/cl{suffix}" if windows else f"{root}/cc",
                "version": "fixture C compiler 1.0",
                "target": module.TARGET_TRIPLES[target],
                "reported_target": reported["c_compiler"],
            },
        },
        "final_linker": {
            "path": f"{root}/Hostx64/x64/link.exe" if windows else f"{root}/rust-lld",
            "version": "fixture linker 1.0",
            "target": module.TARGET_TRIPLES[target],
            "reported_target": reported["linker"],
        },
    }


def test_locked_component_graph_contains_cargo_python_and_all_pnpm_packages():
    module = load_module()
    entries = source_entries(module)
    packages, _relationships = module.component_packages(entries, "0.1.0")
    purls = {
        reference["referenceLocator"]
        for package in packages
        for reference in package["externalRefs"]
    }

    assert "pkg:cargo/axum@0.8.9" in purls
    assert "pkg:pypi/pydantic@2.13.4" in purls
    assert "pkg:npm/react@19.2.8" in purls
    assert "pkg:npm/vite@8.2.1" in purls
    assert "pkg:npm/vitest@4.1.10" in purls
    assert "pkg:npm/%40worldstream/console@0.1.0" in purls
    assert "pkg:cargo/openssl-src@300.6.1%2B3.6.3" in purls
    assert "pkg:cargo/wasi@0.14.7%2Bwasi-0.2.4" in purls
    assert all("+" not in purl for purl in purls)
    assert len(module.pnpm_locked_packages(entries["pnpm-lock.yaml"])) == 95
    by_name = {package["name"]: package for package in packages}
    assert by_name["pydantic"]["downloadLocation"] == "https://pypi.org/simple"
    assert by_name["worldstream-sdk"]["downloadLocation"] == module.REPOSITORY


def test_third_party_notice_bundle_exactly_covers_locked_and_base_components():
    module = load_module()
    entries = source_entries(module)
    declared = module.validate_third_party_notices(entries)
    manifest = module.strict_json(
        entries[module.THIRD_PARTY_NOTICE_MANIFEST_PATH], "notice manifest"
    )

    assert sum(ecosystem == "cargo" for ecosystem, _name, _version in declared) == 240
    assert sum(ecosystem == "npm" for ecosystem, _name, _version in declared) == 95
    assert sum(ecosystem == "apk" for ecosystem, _name, _version in declared) == 15
    assert manifest["inputs"] == {
        "Cargo.lock": "sha256:" + hashlib.sha256(entries["Cargo.lock"]).hexdigest(),
        module.OCI_BASE_IMAGE_PATH: (
            "sha256:" + hashlib.sha256(entries[module.OCI_BASE_IMAGE_PATH]).hexdigest()
        ),
        "pnpm-lock.yaml": (
            "sha256:" + hashlib.sha256(entries["pnpm-lock.yaml"]).hexdigest()
        ),
        "spdx_license_list_revision": module.SPDX_LICENSE_LIST_REVISION,
    }
    assert manifest["components"]["oci_base"]["installed_database_sha256"].startswith(
        "sha256:"
    )
    assert any(
        row["declared_license"] == "MIT/Apache-2.0"
        for row in manifest["components"]["cargo"]
    )
    for material in (
        module.THIRD_PARTY_NOTICE_GENERATOR_PATH,
        module.THIRD_PARTY_NOTICE_MANIFEST_PATH,
        module.THIRD_PARTY_NOTICE_TEXT_PATH,
    ):
        assert material in module.PINNED_MATERIAL_PATHS

    drifted_lock = dict(entries)
    drifted_lock["Cargo.lock"] += b"\n"
    with pytest.raises(module.IdentityError, match="notice input identity"):
        module.validate_third_party_notices(drifted_lock)

    drifted_text = dict(entries)
    drifted_text[module.THIRD_PARTY_NOTICE_TEXT_PATH] += b"tampered\n"
    with pytest.raises(module.IdentityError, match="notice text identity"):
        module.validate_third_party_notices(drifted_text)

    missing_component = copy.deepcopy(manifest)
    missing_component["components"]["npm"].pop()
    drifted_inventory = dict(entries)
    drifted_inventory[module.THIRD_PARTY_NOTICE_MANIFEST_PATH] = module.canonical_json(
        missing_component
    )
    with pytest.raises(module.IdentityError, match="npm notice coverage"):
        module.validate_third_party_notices(drifted_inventory)


def test_spdx_preserves_legacy_license_text_and_models_every_apk_package():
    module = load_module()
    entries = source_entries(module)
    identity = source_identity(module, entries)
    packages, relationships, _describes = module.spdx_graph(
        version="0.1.0",
        revision=revision(),
        subjects={"worldstream-0.1.0-oci-linux-amd64.oci.tar": Path("unused")},
        identities={"source-archive": identity},
        source_entries=entries,
    )
    notice_manifest = module.strict_json(
        entries[module.THIRD_PARTY_NOTICE_MANIFEST_PATH], "notice manifest"
    )
    known = set(notice_manifest["license_texts"])
    legacy = next(
        package
        for package in packages
        if package["name"] == "bitflags" and package["versionInfo"] == "1.3.2"
    )
    assert legacy["licenseDeclared"] == "MIT OR Apache-2.0"
    assert "Upstream declared license: MIT/Apache-2.0." in legacy["attributionTexts"][0]

    apk_packages = [
        package
        for package in packages
        if package["SPDXID"].startswith("SPDXRef-OCIBaseComponent-")
    ]
    assert len(apk_packages) == 15
    assert {package["name"] for package in apk_packages} == {
        row["name"] for row in notice_manifest["components"]["oci_base"]["packages"]
    }
    assert any(
        reference["referenceLocator"] == "pkg:apk/alpine/busybox@1.37.0-r14?arch=x86_64"
        for package in apk_packages
        for reference in package["externalRefs"]
    )
    for package in packages:
        declared = package.get("licenseDeclared")
        if declared not in {None, "NONE", "NOASSERTION"}:
            assert module.spdx_license_expression(declared, known | {"Apache-2.0"}) == (
                declared
            )
    base_id = "SPDXRef-WorldStream-OCI-Base"
    assert {
        relationship["relatedSpdxElement"]
        for relationship in relationships
        if relationship["spdxElementId"] == base_id
        and relationship["relationshipType"] == "CONTAINS"
    } == {package["SPDXID"] for package in apk_packages}


def test_source_package_manifests_reject_duplicate_identity_keys():
    module = load_module()
    entries = source_entries(module)

    duplicate_toolchain = dict(entries)
    duplicate_toolchain["package.json"] = duplicate_toolchain["package.json"].replace(
        f'  "packageManager": "{module.PNPM_PACKAGE_MANAGER}",\n'.encode(),
        b'  "packageManager": "pnpm@0.0.0",\n'
        + f'  "packageManager": "{module.PNPM_PACKAGE_MANAGER}",\n'.encode(),
        1,
    )
    with pytest.raises(module.IdentityError, match="duplicate key 'packageManager'"):
        module.toolchains_from_materials(duplicate_toolchain)

    duplicate_component = dict(entries)
    duplicate_component["web/console/package.json"] = duplicate_component[
        "web/console/package.json"
    ].replace(
        b'    "node": "24.18.1"\n',
        b'    "node": "0.0.0",\n    "node": "24.18.1"\n',
        1,
    )
    with pytest.raises(module.IdentityError, match="duplicate key 'node'"):
        module.component_packages(duplicate_component, "0.1.0")


def test_strict_json_enforces_exact_byte_boundary(monkeypatch, tmp_path: Path):
    module = load_module()
    document = b'{"value":"bounded"}'
    maximum = len(document)
    monkeypatch.setattr(module, "MAX_RELEASE_JSON_BYTES", maximum)

    assert module.strict_json(document, "boundary JSON") == {"value": "bounded"}
    with pytest.raises(module.IdentityError, match="exceeds"):
        module.strict_json(document + b" ", "oversized JSON")

    exact = tmp_path / "exact.json"
    exact.write_bytes(document)
    assert module.regular_bytes(exact, "exact JSON") == document
    oversized = tmp_path / "oversized.json"
    oversized.write_bytes(document + b" ")
    with pytest.raises(module.IdentityError, match="too large"):
        module.regular_bytes(oversized, "oversized JSON")


@pytest.mark.parametrize(
    "constant", [b"NaN", b"Infinity", b"-Infinity", b"1e9999", b"-1e9999"]
)
def test_strict_json_rejects_nested_nonfinite_numbers(constant: bytes):
    module = load_module()

    with pytest.raises(module.IdentityError, match="not strict JSON"):
        module.strict_json(
            b'{"nested":{"values":[' + constant + b"]}}",
            "non-finite JSON",
        )


def test_strict_json_translates_excessive_nesting_to_identity_error():
    module = load_module()
    document = b'{"nested":' + (b"[" * 100_000) + b"0" + (b"]" * 100_000) + b"}"

    with pytest.raises(module.IdentityError, match="not strict JSON"):
        module.strict_json(document, "deep JSON")


def test_oci_package_urls_use_digest_versions_and_canonical_qualifiers():
    module = load_module()
    digest = "a" * 64

    assert module.docker_repository_url("alpine") == "docker.io/library/alpine"
    assert module.docker_repository_url("moby/buildkit") == "docker.io/moby/buildkit"
    assert module.docker_repository_url("docker/dockerfile:1.7") == (
        "docker.io/docker/dockerfile:1.7"
    )
    assert module.oci_purl(f"alpine@sha256:{digest}") == (
        f"pkg:oci/alpine@sha256:{digest}?repository_url=docker.io%2Flibrary%2Falpine"
    )
    assert module.oci_purl(f"moby/buildkit@sha256:{digest}") == (
        f"pkg:oci/buildkit@sha256:{digest}?repository_url=docker.io%2Fmoby%2Fbuildkit"
    )
    assert module.oci_purl(f"docker/dockerfile:1.7@sha256:{digest}") == (
        f"pkg:oci/dockerfile@sha256:{digest}"
        "?repository_url=docker.io%2Fdocker%2Fdockerfile&tag=1.7"
    )
    source_revision = "b" * 40
    assert module.product_purl("0.1.0", source_revision) == (
        "pkg:generic/worldstream@0.1.0?"
        "vcs_url=git%2Bhttps:%2F%2Fgithub.com%2Fimom39a%2Fworldstream%40"
        + source_revision
    )


@pytest.mark.parametrize(
    ("ecosystem", "name", "version"),
    [
        ("cargo", "bad/name", "1.0.0"),
        ("npm", "@scope//name", "1.0.0"),
        ("python", "valid-name", "bad/version"),
    ],
)
def test_component_package_urls_reject_malformed_inputs(
    ecosystem: str, name: str, version: str
):
    module = load_module()
    with pytest.raises(module.IdentityError, match="package-url"):
        module.component_purl(ecosystem, name, version)


def test_uv_is_an_exact_pinned_material_and_toolchain():
    module = load_module()
    entries = source_entries(module)
    value = source_identity(module, entries)

    assert value["toolchains"]["uv"] == {"version": "0.12.5", "pin": ".uv-version"}
    assert (
        value["materials"][".uv-version"]
        == "sha256:" + hashlib.sha256(entries[".uv-version"]).hexdigest()
    )
    assert ".github/workflows/compatibility-gates.yml" in value["materials"]
    assert module.BUILD_TYPE_PATH in value["materials"]


def test_every_repo_local_pre_sign_dynamic_import_is_a_pinned_material():
    module = load_module()
    expected = {
        "scripts/package.py",
        "scripts/release-evidence-assemble.py",
        "scripts/release-evidence-collect.py",
        "scripts/release-evidence-produce.py",
        "scripts/release_build_identity.py",
        "scripts/verify-oci-layout.py",
    }
    assert set(module.PRE_SIGN_DYNAMIC_IMPORT_PATHS) == expected
    assert expected <= set(module.PINNED_MATERIAL_PATHS)

    importers = (
        "scripts/package.py",
        "scripts/release-evidence-assemble.py",
        "scripts/release-evidence-collect.py",
        "scripts/release-evidence-produce.py",
        "scripts/release-supply-chain.py",
    )
    discovered = set()
    for importer in importers:
        source = (ROOT / importer).read_text(encoding="utf-8")
        assert "spec_from_file_location" in source
        for candidate in expected:
            if Path(candidate).name in source:
                discovered.add(candidate)
    assert discovered == expected


def test_pnpm_tarball_integrity_is_bound_in_identity_spdx_slsa_and_workflow():
    module = load_module()
    entries = source_entries(module)
    value = source_identity(module, entries)
    expected_pin = f"package.json#packageManager={module.PNPM_PACKAGE_MANAGER}"

    assert value["toolchains"]["pnpm"] == {
        "version": module.PNPM_VERSION,
        "pin": expected_pin,
    }
    assert value["materials"]["package.json"] == (
        "sha256:" + hashlib.sha256(entries["package.json"]).hexdigest()
    )
    assert module.pinned_toolchain_dependencies() == [
        {
            "uri": module.ZIG_LINUX_X86_64_URL,
            "digest": {"sha256": module.ZIG_LINUX_X86_64_SHA256},
        },
        {
            "uri": module.PNPM_TARBALL_URL,
            "digest": {"sha512": module.PNPM_SHA512},
        },
    ]

    packages, _relationships, _describes = module.spdx_graph(
        version="0.1.0",
        revision=revision(),
        subjects={"worldstream-0.1.0-oci-linux-amd64.oci.tar": Path("unused")},
        identities={"source-archive": value},
        source_entries=entries,
    )
    pnpm = next(package for package in packages if package["name"] == "pnpm")
    assert pnpm["versionInfo"] == module.PNPM_VERSION
    assert pnpm["downloadLocation"] == module.PNPM_TARBALL_URL
    assert pnpm["checksums"] == [
        {"algorithm": "SHA512", "checksumValue": module.PNPM_SHA512}
    ]
    assert pnpm["comment"] == expected_pin

    drifted_package = dict(entries)
    drifted_package["package.json"] = drifted_package["package.json"].replace(
        module.PNPM_SHA512.encode(), b"0" * 128, 1
    )
    with pytest.raises(module.IdentityError, match="toolchain pins"):
        module.toolchains_from_materials(drifted_package)

    drifted_workflow = dict(entries)
    drifted_workflow[module.WORKFLOW_PATH] = drifted_workflow[
        module.WORKFLOW_PATH
    ].replace(
        b"          corepack install\n", b"          corepack install pnpm@11.19.0\n", 1
    )
    with pytest.raises(module.IdentityError, match="pnpm integrity-pinned"):
        module.validate_workflow_producer_contract(drifted_workflow)


def test_workflow_actions_require_exact_commit_tokens():
    module = load_module()
    entries = source_entries(module)
    dependencies = module.workflow_action_dependencies(entries)
    assert dependencies
    assert all(
        set(item) == {"uri", "digest"}
        and set(item["digest"]) == {"gitCommit"}
        and len(item["digest"]["gitCommit"]) == 40
        for item in dependencies
    )

    drifted = dict(entries)
    drifted[module.WORKFLOW_PATH] = drifted[module.WORKFLOW_PATH].replace(
        b"actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
        b"actions/checkout@11d5960a326750d5838078e36cf38b85af677262${{ inputs.ref }}",
        1,
    )
    with pytest.raises(module.IdentityError, match="noncanonical action reference"):
        module.workflow_action_dependencies(drifted)

    flow_step = dict(entries)
    flow_step[module.WORKFLOW_PATH] = flow_step[module.WORKFLOW_PATH].replace(
        b"      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n",
        b"      - {uses: actions/checkout@v4}\n",
        1,
    )
    with pytest.raises(
        module.IdentityError, match="noncanonical (?:action reference|step mapping)"
    ):
        module.workflow_action_dependencies(flow_step)

    for replacement in (
        b'      - "uses": actions/checkout@v4\n',
        b'      - "us\\u0065s": actions/checkout@v4\n',
        b"      - ? uses\n        : actions/checkout@v4\n",
        b"      - <<: *unpinned-action\n",
    ):
        noncanonical_key = dict(entries)
        noncanonical_key[module.WORKFLOW_PATH] = noncanonical_key[
            module.WORKFLOW_PATH
        ].replace(
            b"      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n",
            replacement,
            1,
        )
        with pytest.raises(module.IdentityError, match="noncanonical step mapping"):
            module.workflow_action_dependencies(noncanonical_key)

    hidden_job = dict(entries)
    hidden_job[module.WORKFLOW_PATH] += (
        b'  "hidden-action-job":\n'
        b"    runs-on: ubuntu-24.04\n"
        b"    steps:\n"
        b'      - "us\\u0065s": attacker/action@v1\n'
    )
    with pytest.raises(module.IdentityError, match="noncanonical job mapping"):
        module.workflow_action_dependencies(hidden_job)


def test_release_workflow_routes_and_payload_commands_are_bound():
    module = load_module()
    entries = source_entries(module)
    module.validate_workflow_producer_contract(entries)

    route_drift = dict(entries)
    route_drift[module.WORKFLOW_PATH] = (
        route_drift[module.WORKFLOW_PATH]
        .replace(
            b"  conformance-release:\n",
            b"  conformance-release:\n",
            1,
        )
        .replace(
            b"    runs-on: ubuntu-24.04\n    timeout-minutes: 120",
            b"    runs-on: ubuntu-latest\n    timeout-minutes: 120",
            1,
        )
    )
    with pytest.raises(module.IdentityError, match="producer route drifted"):
        module.validate_workflow_producer_contract(route_drift)

    route_comment_spoof = dict(entries)
    route_comment_spoof[module.WORKFLOW_PATH] = (
        route_comment_spoof[module.WORKFLOW_PATH]
        .replace(
            b"  release-evidence:\n",
            b"  release-evidence:\n",
            1,
        )
        .replace(
            b"    runs-on: ubuntu-24.04\n    timeout-minutes: 240",
            b"    runs-on: ubuntu-latest\n"
            b"    # runs-on: ubuntu-24.04\n"
            b"    timeout-minutes: 240",
            1,
        )
    )
    with pytest.raises(module.IdentityError, match="producer route drifted"):
        module.validate_workflow_producer_contract(route_comment_spoof)

    command_drift = dict(entries)
    command_drift[module.WORKFLOW_PATH] = command_drift[module.WORKFLOW_PATH].replace(
        b"cargo build --release --locked --target x86_64-pc-windows-msvc --workspace",
        b"cargo build --release --target x86_64-pc-windows-msvc --workspace",
        1,
    )
    with pytest.raises(module.IdentityError, match="payload (?:command|step)"):
        module.validate_workflow_producer_contract(command_drift)

    comment_spoof = dict(entries)
    comment_spoof[module.WORKFLOW_PATH] = comment_spoof[module.WORKFLOW_PATH].replace(
        b"cargo build --release --locked --target x86_64-pc-windows-msvc --workspace",
        b"# cargo build --release --locked --target x86_64-pc-windows-msvc --workspace\n"
        b"          cargo build --release --target x86_64-pc-windows-msvc --workspace",
        1,
    )
    with pytest.raises(module.IdentityError, match="payload (?:command|step)"):
        module.validate_workflow_producer_contract(comment_spoof)

    buildkit_drift = dict(entries)
    buildkit_drift[module.WORKFLOW_PATH] = buildkit_drift[module.WORKFLOW_PATH].replace(
        module.BUILDKIT_IMAGE.encode(), b"moby/buildkit:buildx-stable-1", 1
    )
    with pytest.raises(module.IdentityError, match="BuildKit image pin|OCI setup step"):
        module.validate_workflow_producer_contract(buildkit_drift)

    buildkit_comment_spoof = dict(entries)
    buildkit_comment_spoof[module.WORKFLOW_PATH] = buildkit_comment_spoof[
        module.WORKFLOW_PATH
    ].replace(
        f"            image={module.BUILDKIT_IMAGE}".encode(),
        (
            "            image=moby/buildkit:buildx-stable-1\n"
            f"            # image={module.BUILDKIT_IMAGE}"
        ).encode(),
        1,
    )
    with pytest.raises(module.IdentityError, match="BuildKit|OCI setup step"):
        module.validate_workflow_producer_contract(buildkit_comment_spoof)

    frontend_drift = dict(entries)
    frontend_drift["packaging/oci/Dockerfile"] = frontend_drift[
        "packaging/oci/Dockerfile"
    ].replace(module.DOCKERFILE_FRONTEND.encode(), b"docker/dockerfile:1.7", 1)
    with pytest.raises(module.IdentityError, match="Dockerfile frontend image pin"):
        module.build_identity(
            target="oci-linux-amd64",
            revision=revision(),
            source_entries=frontend_drift,
            source_date_epoch=0,
            manifest_sha256=hashlib.sha256(
                frontend_drift["compatibility.json"]
            ).hexdigest(),
            base_image=module.expected_base_image(frontend_drift),
        )


def test_payload_step_control_flow_cannot_be_disabled_or_spoofed():
    module = load_module()
    entries = source_entries(module)
    workflow = entries[module.WORKFLOW_PATH]
    mutations = []

    disabled = workflow.replace(
        b"        if: ${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'native-linux-x86_64' }}\n        shell: bash\n",
        b"        if: ${{ false }}\n        shell: bash\n",
        1,
    )
    mutations.append(disabled)
    renamed = workflow.replace(
        b"      - name: Build Windows release archive\n",
        b"      - name: Disabled Windows release archive\n",
        1,
    )
    mutations.append(renamed)
    wrong_shell = workflow.replace(
        b"      - name: Build and test OCI release image\n        if: ${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'oci-linux-amd64' }}\n        shell: bash\n",
        b"      - name: Build and test OCI release image\n        if: ${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'oci-linux-amd64' }}\n        shell: pwsh\n",
        1,
    )
    mutations.append(wrong_shell)
    dead_code = workflow.replace(
        b"          cargo build --release --locked --target x86_64-unknown-linux-musl --workspace\n",
        b"          if false; then\n          cargo build --release --locked --target x86_64-unknown-linux-musl --workspace\n          fi\n",
        1,
    )
    mutations.append(dead_code)
    disabled_setup = workflow.replace(
        b"      - name: Configure pinned docker-container Buildx for OCI export\n        if: ${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'oci-linux-amd64' }}\n",
        b"      - name: Configure pinned docker-container Buildx for OCI export\n        if: ${{ false }}\n",
        1,
    )
    mutations.append(disabled_setup)
    implicit_archiver = workflow.replace(
        b'          export AR_x86_64_unknown_linux_musl="$(realpath scripts/zig-musl-ar.sh)"\n',
        b"          unset AR_x86_64_unknown_linux_musl\n",
        1,
    )
    mutations.append(implicit_archiver)
    wrong_zig_archive = workflow.replace(
        module.ZIG_LINUX_X86_64_SHA256.encode(), b"0" * 64, 1
    )
    mutations.append(wrong_zig_archive)

    for mutation in mutations:
        drifted = dict(entries)
        drifted[module.WORKFLOW_PATH] = mutation
        with pytest.raises(module.IdentityError, match="payload step|OCI setup step"):
            module.validate_workflow_producer_contract(drifted)


def test_uv_component_graph_rejects_unknown_source_shapes():
    module = load_module()
    entries = source_entries(module)
    drifted = dict(entries)
    drifted["sdk/python/uv.lock"] = drifted["sdk/python/uv.lock"].replace(
        b'source = { registry = "https://pypi.org/simple" }',
        b'source = { directory = "../outside" }',
        1,
    )

    with pytest.raises(module.IdentityError, match="unsupported source identity"):
        module.component_packages(drifted, "0.1.0")


@pytest.mark.parametrize(
    "lockfile",
    [
        b"lockfileVersion: '9.0'\npackages:\n\nsnapshots:\n",
        b"lockfileVersion: '9.0'\npackages:\n\n  react@floating:\n\nsnapshots:\n",
        b"lockfileVersion: '8.0'\npackages:\n\n  react@19.2.8:\n\nsnapshots:\n",
    ],
)
def test_pnpm_component_graph_rejects_empty_or_malformed_lockfiles(lockfile: bytes):
    module = load_module()
    with pytest.raises(module.IdentityError, match="pnpm"):
        module.pnpm_locked_packages(lockfile)


@pytest.mark.parametrize("material", ["pnpm-lock.yaml", ".uv-version"])
def test_build_identity_rejects_locked_toolchain_or_component_drift(material: str):
    module = load_module()
    entries = source_entries(module)
    value = source_identity(module, entries)
    drifted = dict(entries)
    drifted[material] += b"# drift\n"

    with pytest.raises(module.IdentityError):
        module.validate_build_identity(
            value,
            target="source",
            source_entries=drifted,
            revision=revision(),
            source_date_epoch=0,
            manifest_sha256=hashlib.sha256(entries["compatibility.json"]).hexdigest(),
        )


def test_build_identity_missing_material_fails_with_identity_error():
    module = load_module()
    entries = source_entries(module)
    entries.pop("pnpm-lock.yaml")

    with pytest.raises(module.IdentityError, match="missing pinned source materials"):
        source_identity(module, entries)


def test_build_identity_rejects_fabricated_runner_compiler_arguments_and_oci_base():
    module = load_module()
    entries = source_entries(module)
    manifest_sha = hashlib.sha256(entries["compatibility.json"]).hexdigest()
    base = module.expected_base_image(entries)
    value = module.build_identity(
        target="oci-linux-amd64",
        revision=revision(),
        source_entries=entries,
        source_date_epoch=0,
        manifest_sha256=manifest_sha,
        base_image=base,
    )
    assert value["toolchains"]["buildx"] == {
        "version": "0.36.1",
        "pin": (
            ".github/workflows/compatibility-gates.yml#native.setup-buildx.version"
        ),
    }
    assert value["toolchains"]["zig"] == {
        "version": module.ZIG_VERSION,
        "pin": (
            f"{module.ZIG_LINUX_X86_64_URL}#"
            f"sha256:{module.ZIG_LINUX_X86_64_SHA256};"
            f"size:{module.ZIG_LINUX_X86_64_SIZE}"
        ),
    }
    mutations = []
    runner = copy.deepcopy(value)
    runner["target"]["runner"] = "self-hosted"
    mutations.append(runner)
    compiler = copy.deepcopy(value)
    compiler["compiler"]["arguments"].append("--offline")
    mutations.append(compiler)
    arguments = copy.deepcopy(value)
    arguments["arguments"]["cargo"].append("--all-features")
    mutations.append(arguments)
    oci = copy.deepcopy(value)
    oci["oci"]["base_image"] = "alpine@sha256:" + "0" * 64
    mutations.append(oci)
    buildkit = copy.deepcopy(value)
    buildkit["oci"]["buildkit_image"] = "moby/buildkit@sha256:" + "0" * 64
    mutations.append(buildkit)
    frontend = copy.deepcopy(value)
    frontend["oci"]["dockerfile_frontend"] = "docker/dockerfile:1.7@sha256:" + "0" * 64
    mutations.append(frontend)
    buildx = copy.deepcopy(value)
    buildx["toolchains"]["buildx"]["version"] = "0.36.0"
    mutations.append(buildx)

    for mutation in mutations:
        with pytest.raises(module.IdentityError, match="exact canonical"):
            module.validate_build_identity(
                mutation,
                target="oci-linux-amd64",
                source_entries=entries,
                revision=revision(),
                source_date_epoch=0,
                manifest_sha256=manifest_sha,
                base_image=base,
            )


@pytest.mark.parametrize(
    "target", ["source", "linux-x86_64", "windows-x64", "oci-linux-amd64"]
)
def test_observed_payload_runner_and_native_sqlite_tools_are_exact(target: str):
    module = load_module()
    environment = hosted_build_environment(module, target)
    module.validate_observed_build_environment(
        environment,
        target=target,
        expected_rustc_version="1.97.1",
        require_hosted=True,
    )
    assert (
        module.observed_build_environment_from_label(
            module.observed_build_environment_label(environment)
        )
        == environment
    )

    drifted = copy.deepcopy(environment)
    drifted["runner"]["image_version"] = "latest"
    with pytest.raises(module.IdentityError, match="runner facts"):
        module.validate_observed_build_environment(
            drifted,
            target=target,
            expected_rustc_version="1.97.1",
            require_hosted=True,
        )
    if target != "source":
        mutations = []
        missing_compiler = copy.deepcopy(environment)
        missing_compiler["bundled_sqlite"].pop("c_compiler")
        mutations.append(missing_compiler)
        missing_archiver = copy.deepcopy(environment)
        missing_archiver["bundled_sqlite"].pop("archiver")
        mutations.append(missing_archiver)
        relative_linker = copy.deepcopy(environment)
        relative_linker["final_linker"]["path"] = "linker"
        mutations.append(relative_linker)
        wrong_target = copy.deepcopy(environment)
        wrong_target["bundled_sqlite"]["c_compiler"]["target"] = "host"
        mutations.append(wrong_target)
        asserted_target = copy.deepcopy(environment)
        asserted_target["final_linker"]["reported_target"] = "unknown"
        mutations.append(asserted_target)
        wrong_archiver = copy.deepcopy(environment)
        wrong_archiver["bundled_sqlite"]["archiver"]["reported_target"] = "unknown"
        mutations.append(wrong_archiver)
        wrong_rustc = copy.deepcopy(environment)
        wrong_rustc["rustc"]["version"] = "rustc 1.97.2"
        mutations.append(wrong_rustc)
        for mutation in mutations:
            with pytest.raises(module.IdentityError):
                module.validate_observed_build_environment(
                    mutation,
                    target=target,
                    expected_rustc_version="1.97.1",
                    require_hosted=True,
                )


def test_observed_environment_capture_requires_complete_trusted_runner(monkeypatch):
    module = load_module()
    environment = {
        "GITHUB_ACTIONS": "true",
        "GITHUB_REPOSITORY": "imom39a/worldstream",
        "GITHUB_WORKFLOW_REF": (
            "imom39a/worldstream/.github/workflows/compatibility-gates.yml@refs/heads/main"
        ),
        "RUNNER_OS": "Linux",
        "RUNNER_ARCH": "X64",
        "ImageOS": "ubuntu24",
        "ImageVersion": "20260817.1.0",
    }
    for name, value in environment.items():
        monkeypatch.setenv(name, value)
    assert module.capture_observed_build_environment(
        "source", rustc_version="1.97.1"
    ) == hosted_build_environment(module, "source")

    monkeypatch.delenv("ImageVersion")
    with pytest.raises(module.IdentityError, match="runner identity"):
        module.capture_observed_build_environment("source", rustc_version="1.97.1")


def test_release_invocation_parameters_are_observed_and_fail_closed(monkeypatch):
    module = load_module()
    environment = {
        "GITHUB_REPOSITORY": "imom39a/worldstream",
        "GITHUB_WORKFLOW_REF": (
            "imom39a/worldstream/.github/workflows/compatibility-gates.yml@refs/heads/main"
        ),
        "GITHUB_RUN_ID": "1",
        "GITHUB_RUN_ATTEMPT": "1",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_REF": "refs/heads/main",
        "WORLDSTREAM_RELEASE_INPUT": "true",
    }
    for name, value in environment.items():
        monkeypatch.setenv(name, value)
    assert module.release_invocation_parameters() == {
        "trigger": {
            "event": "workflow_dispatch",
            "ref": "refs/heads/main",
            "inputs": {"release": True},
        },
        "source": {"repository": module.REPOSITORY, "ref": "refs/heads/main"},
    }

    monkeypatch.setenv("WORLDSTREAM_RELEASE_INPUT", "false")
    with pytest.raises(module.IdentityError, match="external invocation"):
        module.release_invocation_parameters()


def test_runner_byproduct_is_an_exact_resource_descriptor():
    module = load_module()
    identity = {
        "provider": "github-actions",
        "os": "Linux",
        "architecture": "X64",
        "image": "ubuntu24",
        "image_version": "20260817.1.0",
    }
    descriptor = module.runner_byproduct(identity)
    details = {
        "builder": {
            "id": (f"{module.REPOSITORY}/{module.WORKFLOW_PATH}@refs/heads/main")
        },
        "metadata": {
            "invocationId": f"{module.REPOSITORY}/actions/runs/1/attempts/1",
        },
        "byproducts": [descriptor],
    }

    module.validate_run_details(details, require_github=True)
    assert json.loads(base64.b64decode(descriptor["content"])) == identity

    old_nonstandard = copy.deepcopy(details)
    old_nonstandard["byproducts"][0] = {
        "name": "runner",
        "content": identity,
    }
    with pytest.raises(module.IdentityError, match="base64 ResourceDescriptor"):
        module.validate_run_details(old_nonstandard, require_github=True)

    wrong_digest = copy.deepcopy(details)
    wrong_digest["byproducts"][0]["digest"]["sha256"] = "0" * 64
    with pytest.raises(module.IdentityError, match="not exact or canonical"):
        module.validate_run_details(wrong_digest, require_github=True)

    wrong_builder = copy.deepcopy(details)
    wrong_builder["builder"]["id"] = (
        f"{module.REPOSITORY}/{module.WORKFLOW_PATH}@refs/heads/feature"
    )
    with pytest.raises(module.IdentityError, match="canonical main workflow"):
        module.validate_run_details(wrong_builder, require_github=True)

    wrong_invocation = copy.deepcopy(details)
    wrong_invocation["metadata"]["invocationId"] += "/forged"
    with pytest.raises(module.IdentityError, match="invocation ID"):
        module.validate_run_details(wrong_invocation, require_github=True)

    wrong_runner = copy.deepcopy(details)
    content = json.loads(base64.b64decode(wrong_runner["byproducts"][0]["content"]))
    content["architecture"] = "ARM64"
    wrong_runner["byproducts"] = [module.runner_byproduct(content)]
    with pytest.raises(module.IdentityError, match="runner facts"):
        module.validate_run_details(wrong_runner, require_github=True)


def test_source_revision_accepts_only_a_clean_commit_bound_checkout(tmp_path):
    module = load_module()
    committed = committed_repository(tmp_path)

    assert (
        module.source_revision(tmp_path, committed, require_clean_checkout=True)
        == committed
    )

    (tmp_path / "tracked.txt").write_text("modified\n", encoding="utf-8")
    with pytest.raises(module.IdentityError, match="clean Git checkout"):
        module.source_revision(tmp_path, committed, require_clean_checkout=True)


def test_source_revision_rejects_untracked_source_bytes(tmp_path):
    module = load_module()
    committed = committed_repository(tmp_path)
    (tmp_path / "untracked.txt").write_text("not committed\n", encoding="utf-8")

    with pytest.raises(module.IdentityError, match="clean Git checkout"):
        module.source_revision(tmp_path, committed, require_clean_checkout=True)


def test_commit_bound_source_inventory_excludes_ignored_bytes(tmp_path):
    module = load_module()
    committed_repository(tmp_path)
    (tmp_path / ".gitignore").write_text(".DS_Store\n", encoding="utf-8")
    git(tmp_path, "add", ".gitignore")
    git(tmp_path, "commit", "--quiet", "-m", "ignore local metadata")
    (tmp_path / ".DS_Store").write_bytes(b"uncommitted ignored bytes")

    assert module.tracked_source_paths(tmp_path) == {".gitignore", "tracked.txt"}


@pytest.mark.parametrize(
    "created",
    [
        "2026-99-99T99:99:99Z",
        "2026-02-29T00:00:00Z",
        "2026-08-22T00:00:00.000Z",
        "2026-08-22T00:00:00+00:00",
    ],
)
def test_spdx_creation_time_requires_a_real_canonical_utc_second(created: str):
    module = load_module()
    with pytest.raises(module.IdentityError, match="creation time"):
        module.validate_spdx_created(created)
