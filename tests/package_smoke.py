#!/usr/bin/env python3
"""Focused smoke tests for deterministic archive mechanics."""

from __future__ import annotations

import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from argparse import Namespace
from pathlib import Path, PurePosixPath

import tomllib

MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts/package.py"
REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("worldstream_package", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load package.py")
PACKAGE = importlib.util.module_from_spec(SPEC)
sys.modules["worldstream_package"] = PACKAGE
SPEC.loader.exec_module(PACKAGE)


def write(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)


def install_pinned_build_materials(
    workspace: Path, manifest_toml: bytes, manifest_json: bytes
) -> None:
    """Mirror the canonical release inputs into a self-contained source fixture."""

    overrides = {
        "compatibility.toml": manifest_toml,
        "compatibility.json": manifest_json,
    }
    for relative in PACKAGE.BUILD_IDENTITY.PINNED_MATERIAL_PATHS:
        content = overrides.get(relative)
        if content is None:
            content = (REPOSITORY_ROOT / relative).read_bytes()
        write(workspace / relative, content)


def fixture_inputs(root: Path) -> dict[str, list[tuple[Path, str]]]:
    paths = {
        "bin": [
            (root / "worldstreamd", b"linux daemon fixture"),
            (root / "worldstreamctl", b"linux ctl fixture"),
        ],
        "ui": [
            (root / "ui/index.html", b"<!doctype html>"),
            (root / "ui/assets/app.js", b"console.log('fixture');"),
        ],
        "sdk": [
            (
                root / "sdk/pyproject.toml",
                b"[project]\nname='worldstream-sdk'\nversion='0.1.0'\n",
            ),
            (root / "sdk/uv.lock", b"version = 1\n"),
            (root / "sdk/README.md", b"fixture SDK\n"),
            (root / "sdk/src/worldstream_sdk/__init__.py", b"__version__ = '0.1.0'\n"),
        ],
        "examples": [
            (root / "examples/heist/client.py", b"print('heist')\n"),
            (
                root / "examples/heist/parity_fixture.json",
                b'{"retained_executor":{"pack_id":"worldstream.fixture","pack_version":"1.0.0","pack_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}\n',
            ),
        ],
        "licenses": [(root / "licenses/Apache-2.0.txt", b"Apache-2.0\n")],
    }
    result: dict[str, list[tuple[Path, str]]] = {key: [] for key in paths}
    for group, group_paths in paths.items():
        for source, content in group_paths:
            write(source, content)
            result[group].append((source, source.relative_to(root).as_posix()))
    # required_inputs() would normally produce these destination prefixes.
    result["bin"] = [(source, f"bin/{source.name}") for source, _ in result["bin"]]
    result["ui"] = [
        (source, f"ui/{source.relative_to(root / 'ui').as_posix()}")
        for source, _ in result["ui"]
    ]
    result["sdk"] = [
        (source, f"sdk/python/{source.relative_to(root / 'sdk').as_posix()}")
        for source, _ in result["sdk"]
    ]
    result["examples"] = [
        (source, f"examples/{source.relative_to(root / 'examples').as_posix()}")
        for source, _ in result["examples"]
    ]
    result["licenses"] = [
        (source, f"licenses/{source.relative_to(root / 'licenses').as_posix()}")
        for source, _ in result["licenses"]
    ]
    return result


def add_fixture_client_identities(
    root: Path,
    inputs: dict[str, list[tuple[Path, str]]],
    manifest: dict,
) -> None:
    identity = PACKAGE.canonical_client_contract_identity_bytes(manifest)
    sdk_identity = root / "sdk/src/worldstream_sdk/compatibility_identity.json"
    ui_identity = root / "ui/compatibility-identity.json"
    write(sdk_identity, identity)
    write(ui_identity, identity)
    inputs["sdk"].append((sdk_identity, PACKAGE.CLIENT_IDENTITY_SDK_PATH))
    inputs["ui"].append((ui_identity, PACKAGE.CLIENT_IDENTITY_UI_PATH))
    # The source archive mirrors the repository layout rather than the
    # assembled archive layout.
    write(
        root / "sdk/python/src/worldstream_sdk/compatibility_identity.json",
        identity,
    )
    write(root / "web/console/public/compatibility-identity.json", identity)
    write(
        root / "ui/assets/app.js",
        PACKAGE.UI_CONSUMED_IDENTITY_MARKER + b"\n" + identity,
    )
    write(
        root / "web/console/src/compatibilityIdentity.ts",
        b'import identitySource from "../public/compatibility-identity.json?raw";\n'
        b"export const CLIENT_CONTRACT_IDENTITY_JSON = identitySource;\n"
        b"export const CLIENT_CONTRACT_IDENTITY = JSON.parse(identitySource);\n",
    )
    write(
        root / "web/console/src/main.tsx",
        b"window.__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__ = CLIENT_CONTRACT_IDENTITY;\n",
    )


def expect_package_error(action, message: str) -> None:
    try:
        action()
    except PACKAGE.PackageError:
        return
    raise AssertionError(message)


def run_wrapper(
    workspace: Path, script: str, *arguments: str
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SOURCE_DATE_EPOCH"] = "0"
    environment["WORLDSTREAM_BUILD_REVISION"] = PACKAGE.BUILD_IDENTITY.source_revision(
        REPOSITORY_ROOT
    )
    return subprocess.run(
        ["bash", str(workspace / "scripts" / script), *arguments],
        cwd=workspace,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )


def wrapper_workspace(
    root: Path, package_manifest_toml: bytes, package_manifest_json: bytes
) -> tuple[Path, Path]:
    """Create a self-contained release-valid workspace for wrapper smoke tests."""

    workspace = root / "wrapper-workspace"
    (workspace / "scripts").mkdir(parents=True)
    shutil.copy2(MODULE_PATH, workspace / "scripts/package.py")
    shutil.copy2(
        REPOSITORY_ROOT / "scripts/release_build_identity.py",
        workspace / "scripts/release_build_identity.py",
    )
    for script in ("package-release.sh", "package-oci.sh", "verify-release.sh"):
        shutil.copy2(
            Path(__file__).resolve().parents[1] / "scripts" / script,
            workspace / "scripts" / script,
        )
    (workspace / "compatibility.toml").write_bytes(package_manifest_toml)
    (workspace / "compatibility.json").write_bytes(package_manifest_json)
    shutil.copytree(
        Path(__file__).resolve().parents[1] / "packaging/oci",
        workspace / "packaging/oci",
    )
    inputs = workspace / "inputs"
    fixture_files = fixture_inputs(inputs)
    add_fixture_client_identities(
        inputs, fixture_files, json.loads(package_manifest_json)
    )
    write(
        workspace / "Cargo.toml",
        b"[workspace]\nmembers = []\n[workspace.package]\nversion = '0.1.0'\n",
    )
    write(workspace / "Cargo.lock", b"# fixture lock\n")
    write(
        workspace / "web/console/package.json",
        b'{"name":"@worldstream/console","version":"0.1.0"}\n',
    )
    shutil.copytree(inputs / "sdk", workspace / "sdk/python")
    shutil.copytree(inputs / "examples", workspace / "examples")
    shutil.copytree(inputs / "licenses", workspace / "licenses")
    write(
        workspace / "web/console/public/compatibility-identity.json",
        PACKAGE.canonical_client_contract_identity_bytes(
            json.loads(package_manifest_json)
        ),
    )
    (workspace / "web/console/src").mkdir(parents=True, exist_ok=True)
    shutil.copy2(
        inputs / "web/console/src/compatibilityIdentity.ts",
        workspace / "web/console/src/compatibilityIdentity.ts",
    )
    shutil.copy2(
        inputs / "web/console/src/main.tsx",
        workspace / "web/console/src/main.tsx",
    )
    install_pinned_build_materials(
        workspace, package_manifest_toml, package_manifest_json
    )
    return workspace, inputs


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="worldstream-package-test-") as temporary:
        root = Path(temporary)
        mountinfo = root / "mountinfo"
        mount_path = (
            root.resolve().as_posix().replace("\\", r"\134").replace(" ", r"\040")
        )
        mountinfo.write_text(
            f"32 24 0:30 / {mount_path} rw,relatime - ext4 fixture rw\n"
        )
        original_mountinfo = PACKAGE.LINUX_MOUNTINFO
        original_platform_system = PACKAGE.platform.system
        PACKAGE.LINUX_MOUNTINFO = mountinfo
        PACKAGE.platform.system = lambda: "Linux"
        try:
            mountinfo.write_text(
                "32 24 0:1 / / rw,relatime - overlay fixture rw\n"
                f"33 32 0:30 / {mount_path} rw,relatime - ext4 fixture rw\n"
            )
            PACKAGE.validate_source_filesystem(
                PACKAGE.TARGETS["linux-x86_64"], [root], dry_run=False
            )
            mountinfo.write_text(
                f"32 24 0:30 / {mount_path} rw,relatime - ext2/ext3 fixture rw\n"
            )
            expect_package_error(
                lambda: PACKAGE.validate_source_filesystem(
                    PACKAGE.TARGETS["linux-x86_64"], [root], dry_run=False
                ),
                "ambiguous ext-family mount identity was accepted",
            )
        finally:
            PACKAGE.LINUX_MOUNTINFO = original_mountinfo
            PACKAGE.platform.system = original_platform_system

        inputs = fixture_inputs(root)
        write(
            root / "Cargo.toml",
            b"[workspace]\nmembers = []\n[workspace.package]\nversion = '0.1.0'\n",
        )
        write(
            root / "web/console/package.json",
            b'{"name":"@worldstream/console","version":"0.1.0"}\n',
        )
        manifest_toml = b"""schema = 'worldstream/storage-compatibility-manifest/v1'
manifest_kind = 'release'
manifest_revision = 1
release_ready = true
release_candidate = '0.1.0'
unresolved_required_fields = []
validation_policy = 'fail_closed'
reviewed_source = 'compatibility.toml'
canonical_mirror = 'compatibility.json'
release_artifact_digest_source = 'detached_release_manifest'

[gate_framework]
validation = 'fail_closed'
unavailable_dependency_policy = 'explicit_skip_or_fail'

[contracts]
product = '0.1.0'
wire = '0.1'
config = 1
storage_schema = 1
core_schema_version = 'worldstream.core-room-state.v1'
hash_suite = 'blake3-canonical-json-v1'

[[pack_executors]]
pack_id = 'worldstream.fixture'
explanatory_version = '1.0.0'
host_contract_id = 'worldstream/activity-pack/v1'
revision_lock_id = 'worldstream/pack-revision-lock/v1'
revision_digest_algorithm = 'blake3'
revision_digest = 'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
descriptor_digest = 'blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
executor_artifact_digest = 'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
schema_bundle_digest = 'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'
codec_bundle_digest = 'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'
golden_corpus_digest = 'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'
selectable_for_new_rooms = true
runnable_for_retained_rooms = true
status = 'resolved'
required_for_release = true

[[platforms]]
id = 'native-linux-x86_64'
support = 'release'
target = 'x86_64-unknown-linux-musl'
storage_profiles = ['sqlite-bundled', 'postgres-primary']

[[platforms]]
id = 'native-windows-x64'
support = 'release'
target = 'x86_64-pc-windows-msvc'
storage_profiles = ['sqlite-bundled', 'postgres-primary']

[[platforms]]
id = 'oci-linux-amd64'
support = 'release'
target = 'linux/amd64'
storage_profiles = ['sqlite-bundled', 'postgres-primary']
persistent_data_path = '/var/lib/worldstream'
read_only_root_compatible = true
sqlite_overlay_allowed = false

[[release_artifacts]]
id = 'source-archive'
profile = 'source'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'native-linux-x86_64-archive'
profile = 'native-linux-x86_64'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'native-windows-x64-archive'
profile = 'native-windows-x64'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'oci-linux-amd64-image'
profile = 'oci-linux-amd64'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'checksums'
profile = 'all'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'sigstore-bundle'
profile = 'all'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'spdx-sbom'
profile = 'all'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'

[[release_artifacts]]
id = 'slsa-provenance'
profile = 'all'
digest_algorithm = 'sha256'
digest = ''
status = 'detached'
digest_location = 'release-manifest.json'
"""
        fixture_evidence_ids = (
            "manifest-syntax-parity",
            "sqlite-conformance-migration-backup-restore-crash",
            "postgresql-direct-and-transaction-pooler-conformance",
            "all-prior-forward-migrations-both-backends",
            "sqlite-postgresql-transfer-byte-parity-and-epoch-fencing",
            "backend-native-isolated-restore-and-full-semantic-verifier",
            "native-linux-release-profile",
            "native-windows-release-profile",
            "oci-linux-amd64-release-profile",
            "macos-source-quickstart",
            "config-secrets-probes-observability-security",
            "checksums-signature-sbom-provenance",
            "failure-fuzz-resource-and-one-hour-sqlite-soak",
            "reference-performance-per-backend",
        )
        manifest_toml += "".join(
            "\n[[evidence]]\n"
            f"id = '{evidence_id}'\n"
            "release_gate = true\n"
            "status = 'detached'\n"
            "artifact_digest = ''\n"
            "artifact_digest_location = 'release-manifest.json'\n"
            for evidence_id in fixture_evidence_ids
        ).encode()
        manifest = tomllib.loads(manifest_toml.decode())
        manifest_json = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
        install_pinned_build_materials(root, manifest_toml, manifest_json)
        add_fixture_client_identities(root, inputs, manifest)
        target = PACKAGE.TARGETS["linux-x86_64"]
        files = PACKAGE.collect_package_files(
            target,
            "0.1.0",
            manifest_toml,
            manifest_json,
            inputs,
            0,
            require_clean_checkout=False,
        )
        first = root / "first" / "worldstream-0.1.0-linux-x86_64.tar.gz"
        second = root / "second" / "worldstream-0.1.0-linux-x86_64.tar.gz"
        PACKAGE.write_tar_gz(first, "worldstream-0.1.0-linux-x86_64", files, 0)
        PACKAGE.write_tar_gz(second, "worldstream-0.1.0-linux-x86_64", files, 0)
        assert first.read_bytes() == second.read_bytes(), (
            "archive bytes are not reproducible"
        )
        PACKAGE.verify_archive(first)
        client_files = dict(files)
        PACKAGE.validate_packaged_client_identities(
            list(client_files.items()), manifest, target
        )
        PACKAGE.validate_packaged_ui_consumed_assets(
            list(client_files.items()), manifest, target
        )
        PACKAGE.validate_packaged_ui_consumed_identity(
            list(client_files.items()), manifest, target
        )
        profile_metadata = json.loads(dict(files)["metadata/profile.json"])
        assert profile_metadata["backend"] == {
            "default_profile": "sqlite-bundled",
            "postgres_primary": {
                "administration": "direct_offline",
                "remote_tls_required": True,
                "runtime_connection_modes": [
                    "direct",
                    "session_pool",
                    "transaction_pool",
                ],
                "secret_inputs": [
                    "owner_readable_secret_file",
                    "inherited_handle",
                ],
            },
            "selection": "startup_fixed",
            "sqlite_bundled": {
                "backup_manifest": "worldstream/backup-manifest/v1",
                "filesystem_scope": "local_only",
                "supported_filesystems": ["ext4", "xfs"],
            },
        }
        assert profile_metadata["paths"]["data"] == {
            "implicit_cwd_or_home_search": False,
            "packaged_payload": False,
            "selection": "explicit_config_storage_data_dir",
        }
        ui_wire_drift = json.loads(json.dumps(manifest))
        ui_wire_drift["contracts"]["wire"] = "0.2"
        ui_wire_drift_files = dict(client_files)
        ui_wire_drift_files["ui/assets/app.js"] = (
            PACKAGE.UI_CONSUMED_IDENTITY_MARKER
            + b"\n"
            + PACKAGE.canonical_client_contract_identity_bytes(ui_wire_drift)
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_ui_consumed_identity(
                list(ui_wire_drift_files.items()), manifest, target
            ),
            "UI-consumed wire drift was accepted with a valid public identity",
        )
        ui_config_drift = json.loads(json.dumps(manifest))
        ui_config_drift["contracts"]["config"] = 2
        ui_config_drift_files = dict(client_files)
        ui_config_drift_files["ui/assets/app.js"] = (
            PACKAGE.UI_CONSUMED_IDENTITY_MARKER
            + b"\n"
            + PACKAGE.canonical_client_contract_identity_bytes(ui_config_drift)
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_ui_consumed_identity(
                list(ui_config_drift_files.items()), manifest, target
            ),
            "UI-consumed config drift was accepted with a valid public identity",
        )
        ui_pack_drift = json.loads(json.dumps(manifest))
        ui_pack_drift["pack_executors"][0]["revision_digest"] = "blake3:" + "0" * 64
        ui_pack_drift_files = dict(client_files)
        ui_pack_drift_files["ui/assets/app.js"] = (
            PACKAGE.UI_CONSUMED_IDENTITY_MARKER
            + b"\n"
            + PACKAGE.canonical_client_contract_identity_bytes(ui_pack_drift)
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_ui_consumed_identity(
                list(ui_pack_drift_files.items()), manifest, target
            ),
            "UI-consumed full pack-row drift was accepted with a valid public identity",
        )
        orphan_fixture = json.loads(client_files[PACKAGE.HEIST_PARITY_FIXTURE_PATH])
        orphan_fixture["retained_executor"]["pack_digest"] = (
            "blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe"
        )
        orphan_fixture_files = dict(client_files)
        orphan_fixture_files[PACKAGE.HEIST_PARITY_FIXTURE_PATH] = (
            json.dumps(orphan_fixture, indent=2, sort_keys=True).encode() + b"\n"
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_ui_consumed_assets(
                list(orphan_fixture_files.items()), manifest, target
            ),
            "orphan retained-Heist pack digest was accepted",
        )
        wire_drift = json.loads(json.dumps(manifest))
        wire_drift["contracts"]["wire"] = "0.2"
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(client_files.items()), wire_drift, target
            ),
            "wire contract drift was accepted",
        )
        config_drift = json.loads(json.dumps(manifest))
        config_drift["contracts"]["config"] = 2
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(client_files.items()), config_drift, target
            ),
            "config contract drift was accepted",
        )
        missing_pack_identity = dict(client_files)
        del missing_pack_identity[PACKAGE.CLIENT_IDENTITY_SDK_PATH]
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(missing_pack_identity.items()), manifest, target
            ),
            "missing SDK client identity was accepted",
        )
        stale_pack_manifest = json.loads(json.dumps(manifest))
        stale_pack_manifest["pack_executors"][0]["explanatory_version"] = "0.9.0"
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(client_files.items()), stale_pack_manifest, target
            ),
            "stale pack row was accepted",
        )
        digest_drift = json.loads(json.dumps(manifest))
        digest_drift["pack_executors"][0]["revision_digest"] = "blake3:" + "0" * 64
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(client_files.items()), digest_drift, target
            ),
            "exact pack revision digest drift was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_client_contract_identity(
                b'{"schema":"broken"}',
                manifest,
                "malformed fixture identity",
            ),
            "malformed client identity was accepted",
        )
        disagreement_manifest = json.loads(json.dumps(manifest))
        disagreement_manifest["contracts"]["wire"] = "0.2"
        disagreement = dict(client_files)
        disagreement[PACKAGE.CLIENT_IDENTITY_UI_PATH] = (
            PACKAGE.canonical_client_contract_identity_bytes(disagreement_manifest)
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_client_identities(
                list(disagreement.items()), manifest, target
            ),
            "SDK/UI client identity disagreement was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_archive_layout(
                ["worldstream-0.1.0-linux-x86_64/extra/payload"],
                "worldstream-0.1.0-linux-x86_64",
                PACKAGE.TARGETS["linux-x86_64"],
            ),
            "unlisted archive layout path was accepted",
        )
        first_modes = PACKAGE.archive_member_modes(first)
        assert all(
            mode == (0o755 if is_directory or "/bin/" in name else 0o644)
            for name, (is_directory, mode) in first_modes.items()
        )
        renamed = root / "renamed.tar.gz"
        shutil.copy2(first, renamed)
        expect_package_error(
            lambda: PACKAGE.verify_archive(renamed),
            "archive with mismatched filename identity was accepted",
        )

        inconsistent_manifest = json.loads(json.dumps(manifest))
        inconsistent_manifest["release_artifacts"][0]["status"] = "resolved"
        inconsistent_manifest["release_artifacts"][0]["digest"] = ""
        expect_package_error(
            lambda: PACKAGE.validate_manifest_shape(inconsistent_manifest),
            "resolved artifact without a digest was accepted",
        )
        detached_with_digest = json.loads(json.dumps(manifest))
        detached_with_digest["release_artifacts"][0]["digest"] = "sha256:" + "0" * 64
        expect_package_error(
            lambda: PACKAGE.validate_manifest_shape(detached_with_digest),
            "detached artifact with an embedded digest was accepted",
        )
        detached_without_location = json.loads(json.dumps(manifest))
        detached_without_location["release_artifacts"][0].pop("digest_location")
        expect_package_error(
            lambda: PACKAGE.validate_manifest_shape(detached_without_location),
            "detached artifact without an external digest location was accepted",
        )

        expect_package_error(
            lambda: PACKAGE.checksums_file([("unsafe\nname", b"payload")]),
            "newline-containing checksum path was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.checksums_file(
                [("duplicate", b"first"), ("duplicate", b"second")]
            ),
            "duplicate checksum path was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.parse_checksums(f"{'0' * 64}  ../outside\n".encode()),
            "traversal checksum path was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.parse_checksums(b"not-a-digest  payload\n"),
            "malformed checksum digest was accepted",
        )
        PACKAGE.validate_component_version_sources(
            version="0.1.0",
            daemon_version_file=root / "Cargo.toml",
            sdk_version_file=root / "sdk/pyproject.toml",
            ui_version_file=root / "web/console/package.json",
        )
        bad_ui_version = root / "web/console/bad-package.json"
        write(
            bad_ui_version,
            b'{"name":"@worldstream/console","version":"0.2.0"}\n',
        )
        expect_package_error(
            lambda: PACKAGE.validate_component_version_sources(
                version="0.1.0",
                daemon_version_file=root / "Cargo.toml",
                sdk_version_file=root / "sdk/pyproject.toml",
                ui_version_file=bad_ui_version,
            ),
            "UI version drift was accepted",
        )
        if os.name != "nt":
            broad_write = root / "broad-write.txt"
            write(broad_write, b"unsafe")
            broad_write.chmod(0o666)
            expect_package_error(
                lambda: PACKAGE.validate_source_file(broad_write, "broad-write.txt"),
                "group/world-writable package input was accepted",
            )
            broad_write.chmod(0o644)
        for unsafe_path in (
            r"bin\worldstreamd.exe",
            r"\\server\share\worldstreamd.exe",
        ):
            expect_package_error(
                lambda unsafe_path=unsafe_path: PACKAGE.validate_package_path(
                    unsafe_path
                ),
                "Windows path separator or UNC package path was accepted",
            )
        PACKAGE.validate_sigstore_shape(
            {
                "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.3",
                "verificationMaterial": {"tlogEntries": [{}]},
                "messageSignature": {
                    "messageDigest": {"algorithm": "SHA2_256", "digest": "encoded"}
                },
            }
        )
        PACKAGE.validate_spdx_shape(
            {
                "spdxVersion": "SPDX-2.3",
                "SPDXID": "SPDXRef-DOCUMENT",
                "name": "worldstream",
                "documentNamespace": "https://example.invalid/worldstream",
                "creationInfo": {
                    "created": "2026-01-01T00:00:00Z",
                    "creators": ["Tool: fixture"],
                },
                "packages": [{"SPDXID": "SPDXRef-Package"}],
                "files": [{"SPDXID": "SPDXRef-File"}],
                "relationships": [
                    {
                        "spdxElementId": "SPDXRef-DOCUMENT",
                        "relationshipType": "DESCRIBES",
                        "relatedSpdxElement": "SPDXRef-Package",
                    }
                ],
                "documentDescribes": ["SPDXRef-Package"],
            }
        )
        PACKAGE.validate_slsa_shape(
            {
                "_type": "https://in-toto.io/Statement/v1",
                "subject": [{"name": "payload", "digest": {"sha256": "0" * 64}}],
                "predicateType": "https://slsa.dev/provenance/v1",
                "predicate": {
                    "buildDefinition": {
                        "externalParameters": {"product": "0.1.0"},
                        "internalParameters": {"toolchains": {"rustc": "1.97.1"}},
                        "resolvedDependencies": [
                            {
                                "uri": "git+https://example.invalid/worldstream",
                                "digest": {"gitCommit": "0" * 40},
                            }
                        ],
                    },
                    "runDetails": {},
                },
            }
        )
        expect_package_error(
            lambda: PACKAGE.validate_release_artifact_path(
                "worldstream-0.1.0-linux-arm64.tar.gz",
                "native-linux-x86_64-archive",
                "0.1.0",
            ),
            "unsupported ARM64 artifact was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_artifact_paths(
                [("ui/assets/worldstream-arm64.js", b"unsupported")], target
            ),
            "unsupported platform payload path was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_artifact_paths(
                [("source/data/worldstream.sqlite3", b"runtime state")],
                PACKAGE.TARGETS["source"],
            ),
            "runtime data payload was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_sigstore_shape({"bundleVersion": "0.3"}),
            "weak Sigstore shape was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_spdx_shape(
                {"spdxVersion": "SPDX-2.3", "SPDXID": "SPDXRef-DOCUMENT"}
            ),
            "incomplete SPDX shape was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.validate_slsa_shape(
                {"predicateType": "https://slsa.dev/provenance/v1", "subject": []}
            ),
            "incomplete SLSA shape was accepted",
        )
        expect_package_error(
            lambda: PACKAGE.oci_context(
                Namespace(
                    output=str(root / "bad-oci"),
                    binary_dir=str(root),
                    ui_dir=str(root / "ui"),
                    sdk_dir=str(root / "sdk"),
                    examples_dir=str(root / "examples"),
                    licenses_dir=str(root / "licenses"),
                    base_image="alpine:latest",
                    source_date_epoch="0",
                    dry_run=False,
                )
            ),
            "unpinned OCI base image was accepted",
        )
        evidence = root / "dist" / "evidence"
        evidence.mkdir(parents=True)
        write(
            evidence / "release-manifest.json",
            json.dumps(
                {
                    "product": "0.1.0",
                    "artifacts": {"native-linux-x86_64-archive": "missing.tar.gz"},
                }
            ).encode()
            + b"\n",
        )
        expect_package_error(
            lambda: PACKAGE.verify_release_directory(evidence, structural_only=False),
            "missing release artifact was accepted",
        )

        # A complete document-only evidence bundle is structurally accepted,
        # but the verifier must still return its explicit incomplete status
        # when cosign is unavailable.
        valid_evidence = root / "dist" / "valid-evidence"
        valid_evidence.mkdir(parents=True)
        artifact_paths = {
            "source-archive": "worldstream-0.1.0-source.tar.gz",
            "native-linux-x86_64-archive": "worldstream-0.1.0-linux-x86_64.tar.gz",
            "native-windows-x64-archive": "worldstream-0.1.0-windows-x64.zip",
            "oci-linux-amd64-image": "worldstream-0.1.0-oci-linux-amd64.oci.tar",
            "checksums": "SHA256SUMS",
            "sigstore-bundle": "sigstore.bundle.json",
            "spdx-sbom": "sbom.spdx.json",
            "slsa-provenance": "provenance.json",
        }

        windows_inputs = {group: list(entries) for group, entries in inputs.items()}
        windows_inputs["bin"] = []
        windows_binary_dir = root / "release-fixture-windows-binaries"
        for binary in PACKAGE.TARGETS["windows-x64"].binary_names:
            source = windows_binary_dir / binary
            write(source, f"windows fixture {binary}\n".encode())
            windows_inputs["bin"].append((source, f"bin/{binary}"))
        windows_files = PACKAGE.collect_package_files(
            PACKAGE.TARGETS["windows-x64"],
            "0.1.0",
            manifest_toml,
            manifest_json,
            windows_inputs,
            0,
            require_clean_checkout=False,
        )
        release_windows = (
            root
            / "release-fixture-windows"
            / artifact_paths["native-windows-x64-archive"]
        )
        PACKAGE.write_zip(
            release_windows,
            "worldstream-0.1.0-windows-x64",
            windows_files,
            0,
        )

        repository_root = Path(__file__).resolve().parents[1]
        for relative in PACKAGE.BUILD_IDENTITY.PINNED_MATERIAL_PATHS:
            if relative in {"compatibility.toml", "compatibility.json"}:
                continue
            write(root / relative, (repository_root / relative).read_bytes())
        write(root / "compatibility.toml", manifest_toml)
        write(root / "compatibility.json", manifest_json)
        source_inputs = PACKAGE.required_inputs(
            PACKAGE.TARGETS["source"],
            root,
            root,
            root,
            root,
            root,
            source_dir=root,
        )
        release_source_files = PACKAGE.collect_package_files(
            PACKAGE.TARGETS["source"],
            "0.1.0",
            manifest_toml,
            manifest_json,
            source_inputs,
            0,
            source_root=root,
            source_revision=PACKAGE.BUILD_IDENTITY.source_revision(repository_root),
        )
        release_source = (
            root / "release-fixture-source" / artifact_paths["source-archive"]
        )
        PACKAGE.write_tar_gz(
            release_source,
            "worldstream-0.1.0-source",
            release_source_files,
            0,
        )

        oci_test_path = Path(__file__).resolve().parent / "oci_runtime_smoke.py"
        oci_spec = importlib.util.spec_from_file_location(
            "worldstream_package_smoke_oci_fixture", oci_test_path
        )
        if oci_spec is None or oci_spec.loader is None:
            raise AssertionError("could not load the OCI layout fixture")
        oci_fixture = importlib.util.module_from_spec(oci_spec)
        sys.modules[oci_spec.name] = oci_fixture
        oci_spec.loader.exec_module(oci_fixture)
        oci_fixture_dir = root / "release-fixture-oci"
        oci_fixture_dir.mkdir()
        fixture_source_entries = PACKAGE.BUILD_IDENTITY.source_entries_from_root(root)
        oci_build_identity = PACKAGE.BUILD_IDENTITY.build_identity(
            target="oci-linux-amd64",
            revision=PACKAGE.BUILD_IDENTITY.source_revision(repository_root),
            source_entries=fixture_source_entries,
            source_date_epoch=0,
            manifest_sha256=PACKAGE.sha256_bytes(manifest_json),
            base_image=PACKAGE.BUILD_IDENTITY.expected_base_image(
                fixture_source_entries
            ),
        )
        synthetic_oci, _tested_image_id = oci_fixture.oci_layout_fixture(
            oci_fixture_dir,
            labels={
                "org.opencontainers.image.version": "0.1.0",
                "org.opencontainers.image.revision": oci_build_identity["source"][
                    "revision"
                ],
                "io.worldstream.target": "linux/amd64",
                "io.worldstream.base-image": oci_build_identity["oci"]["base_image"],
                "io.worldstream.build-identity": PACKAGE.BUILD_IDENTITY.build_identity_digest(
                    oci_build_identity
                ),
            },
        )
        payloads = {
            artifact_paths["source-archive"]: release_source.read_bytes(),
            artifact_paths["native-linux-x86_64-archive"]: first.read_bytes(),
            artifact_paths["native-windows-x64-archive"]: release_windows.read_bytes(),
            artifact_paths["oci-linux-amd64-image"]: synthetic_oci.read_bytes(),
        }
        for relative, content in payloads.items():
            write(valid_evidence / relative, content)
        ready_manifest = json.loads(json.dumps(manifest))
        ready_manifest["manifest_kind"] = "release"
        ready_manifest["release_ready"] = True
        ready_manifest["unresolved_required_fields"] = []
        verifier = PACKAGE.release_evidence_verifier()
        collector = verifier.evidence_collector()
        specs = {spec.evidence_id: spec for spec in collector.SOURCE_SPECS}
        supply_chain_evidence_id = "checksums-signature-sbom-provenance"
        pre_sign_subject_payloads = {}
        for evidence_id in fixture_evidence_ids:
            if evidence_id == supply_chain_evidence_id:
                continue
            spec = specs[evidence_id]
            checks = {check: True for check in spec.checks}
            details = {
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
            }
            payload_artifact_id = {
                "native-linux": "native-linux-x86_64-archive",
                "native-windows": "native-windows-x64-archive",
                "oci-linux": "oci-linux-amd64-image",
            }.get(spec.source_id)
            for binding in collector.REQUIRED_ARTIFACT_BINDINGS[spec.source_id]:
                if payload_artifact_id is None:
                    details["artifacts"][binding] = {
                        "sha256": "sha256:" + "a" * 64,
                        "size_bytes": 1,
                    }
                else:
                    relative = artifact_paths[payload_artifact_id]
                    details["artifacts"][binding] = {
                        "sha256": "sha256:" + PACKAGE.sha256_bytes(payloads[relative]),
                        "size_bytes": len(payloads[relative]),
                    }
            source = {
                "schema": spec.schema,
                "source_id": spec.source_id,
                "evidence_id": evidence_id,
                "status": "passed",
                "release_evidence": True,
                "fail_closed": False,
                "version": "0.1.0",
                "platform": spec.platform,
                "contract": ready_manifest["contracts"],
                "checks": checks,
                "details": details,
            }
            source_payload = json.dumps(source, sort_keys=True).encode() + b"\n"
            normalized = {
                "schema": collector.NORMALIZED_SCHEMA,
                "evidence_id": evidence_id,
                "release_gate": True,
                "release_evidence": True,
                "fail_closed": False,
                "status": "passed",
                "summary": {"failures": 0, "incomplete_skips": 0},
                "version": "0.1.0",
                "platform": spec.platform,
                "contract": ready_manifest["contracts"],
                "checks": checks,
                "producer_details": details,
                "source_report": {
                    "source_id": spec.source_id,
                    "evidence_id": evidence_id,
                    "schema": spec.schema,
                    "sha256": "sha256:" + PACKAGE.sha256_bytes(source_payload),
                },
            }
            write(
                valid_evidence / f"evidence/{evidence_id}.json",
                json.dumps(normalized, sort_keys=True).encode() + b"\n",
            )
            pre_sign_subject_payloads[f"supply-chain/subjects/{evidence_id}.json"] = (
                source_payload
            )
        for relative, content in pre_sign_subject_payloads.items():
            write(valid_evidence / relative, content)
        sigstore_fixture = (
            json.dumps(
                {
                    "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.3",
                    "verificationMaterial": {"tlogEntries": [{}]},
                    "messageSignature": {
                        "messageDigest": {
                            "algorithm": "SHA2_256",
                            "digest": "encoded",
                        }
                    },
                }
            ).encode()
            + b"\n"
        )
        write(
            valid_evidence / artifact_paths["sigstore-bundle"],
            sigstore_fixture,
        )
        write(
            valid_evidence / "supply-chain/subject-inventory.bundle.json",
            sigstore_fixture,
        )
        subject_payloads = dict(payloads)
        subject_payloads.update(pre_sign_subject_payloads)
        inventory_subjects = [
            {
                "kind": "payload",
                "id": artifact_id,
                "path": relative,
                "sha256": "sha256:" + PACKAGE.sha256_bytes(payloads[relative]),
                "size_bytes": len(payloads[relative]),
            }
            for artifact_id, relative in artifact_paths.items()
            if relative in payloads
        ]
        inventory_subjects.extend(
            {
                "kind": "source-report",
                "id": PurePosixPath(relative).stem,
                "path": relative,
                "sha256": "sha256:" + PACKAGE.sha256_bytes(content),
                "size_bytes": len(content),
            }
            for relative, content in pre_sign_subject_payloads.items()
        )
        inventory_subjects.sort(key=lambda item: (item["kind"], item["id"]))
        write(
            valid_evidence / "supply-chain/subject-inventory.json",
            json.dumps(
                {
                    "schema": "worldstream/release-subject-inventory/v1",
                    "phase": "pre-sign",
                    "product": "0.1.0",
                    "subjects": inventory_subjects,
                },
                indent=2,
                sort_keys=True,
            ).encode()
            + b"\n",
        )
        subject_paths = {
            relative: valid_evidence / relative for relative in subject_payloads
        }
        payload_paths_by_id = {
            artifact_id: valid_evidence / relative
            for artifact_id, relative in artifact_paths.items()
            if artifact_id in PACKAGE.CHECKSUM_PAYLOAD_ARTIFACT_IDS
        }
        build_identities, release_source_entries = (
            PACKAGE.BUILD_IDENTITY.release_payload_identities(
                payload_paths_by_id, "0.1.0"
            )
        )
        source_revision = build_identities["source-archive"]["source"]["revision"]
        spdx_packages, spdx_relationships, document_describes = (
            PACKAGE.BUILD_IDENTITY.spdx_graph(
                version="0.1.0",
                revision=source_revision,
                subjects=subject_paths,
                identities=build_identities,
                source_entries=release_source_entries,
            )
        )
        spdx_files = [
            {
                "SPDXID": PACKAGE.BUILD_IDENTITY._spdx_id("ReleaseSubject", relative),
                "fileName": relative,
                "checksums": [
                    {
                        "algorithm": "SHA256",
                        "checksumValue": PACKAGE.sha256_bytes(content),
                    }
                ],
                "copyrightText": "NOASSERTION",
                "licenseConcluded": "NOASSERTION",
            }
            for relative, content in sorted(subject_payloads.items())
        ]
        write(
            valid_evidence / artifact_paths["spdx-sbom"],
            json.dumps(
                {
                    "spdxVersion": "SPDX-2.3",
                    "SPDXID": "SPDXRef-DOCUMENT",
                    "name": "worldstream",
                    "dataLicense": "CC0-1.0",
                    "documentNamespace": "https://example.invalid/worldstream",
                    "creationInfo": {
                        "created": "2026-01-01T00:00:00Z",
                        "creators": ["Tool: fixture"],
                    },
                    "packages": spdx_packages,
                    "files": spdx_files,
                    "relationships": spdx_relationships,
                    "documentDescribes": document_describes,
                }
            ).encode()
            + b"\n",
        )
        provenance_subjects = [
            {
                "name": relative,
                "digest": {"sha256": PACKAGE.sha256_bytes(content)},
            }
            for relative, content in sorted(subject_payloads.items())
        ]
        provenance_graph = PACKAGE.BUILD_IDENTITY.provenance_graph(
            version="0.1.0",
            revision=source_revision,
            subjects=payload_paths_by_id,
            identities=build_identities,
            source_entries=release_source_entries,
        )
        write(
            valid_evidence / artifact_paths["slsa-provenance"],
            json.dumps(
                {
                    "_type": "https://in-toto.io/Statement/v1",
                    "subject": provenance_subjects,
                    "predicateType": "https://slsa.dev/provenance/v1",
                    "predicate": {
                        "buildDefinition": {
                            "buildType": (
                                "https://github.com/imom39a/worldstream/"
                                "release-build/v1"
                            ),
                            **provenance_graph,
                        },
                        "runDetails": {
                            "builder": {
                                "id": (
                                    "https://github.com/imom39a/worldstream/"
                                    ".github/workflows/compatibility-gates.yml@refs/heads/main"
                                )
                            },
                            "metadata": {
                                "invocationId": (
                                    "https://github.com/imom39a/worldstream/"
                                    "actions/runs/1/attempts/1"
                                ),
                                "startedOn": "2026-01-01T00:00:00Z",
                                "finishedOn": "2026-01-01T00:00:00Z",
                            },
                            "byproducts": [
                                PACKAGE.BUILD_IDENTITY.runner_byproduct(
                                    {
                                        "provider": "github-actions",
                                        "os": "Linux",
                                        "architecture": "X64",
                                        "image": "ubuntu24",
                                        "image_version": "20260801.1",
                                    }
                                )
                            ],
                        },
                    },
                }
            ).encode()
            + b"\n",
        )
        write(
            valid_evidence / artifact_paths["checksums"],
            PACKAGE.checksums_file(sorted(subject_payloads.items())),
        )
        supply_spec = specs[supply_chain_evidence_id]
        supply_checks = {check: True for check in supply_spec.checks}
        supply_artifact_paths = {
            "subject-inventory": "supply-chain/subject-inventory.json",
            "subject-signature": "supply-chain/subject-inventory.bundle.json",
            "checksums": artifact_paths["checksums"],
            "spdx-sbom": artifact_paths["spdx-sbom"],
            "slsa-provenance": artifact_paths["slsa-provenance"],
        }
        supply_details = {
            "producer_id": collector.EXPECTED_PRODUCER_IDS[supply_spec.source_id],
            "phase": "pre-sign",
            "outcomes": {
                check: {
                    "status": "passed",
                    "observations": [{"kind": "fixture", "value": check}],
                }
                for check in supply_spec.checks
            },
            "artifacts": {
                binding: {
                    "sha256": "sha256:"
                    + PACKAGE.sha256_file(valid_evidence / relative),
                    "size_bytes": (valid_evidence / relative).stat().st_size,
                }
                for binding, relative in supply_artifact_paths.items()
            },
        }
        supply_normalized = {
            "schema": collector.NORMALIZED_SCHEMA,
            "evidence_id": supply_chain_evidence_id,
            "release_gate": True,
            "release_evidence": True,
            "fail_closed": False,
            "status": "passed",
            "summary": {"failures": 0, "incomplete_skips": 0},
            "version": "0.1.0",
            "platform": supply_spec.platform,
            "contract": ready_manifest["contracts"],
            "checks": supply_checks,
            "producer_details": supply_details,
            "source_report": {
                "source_id": supply_spec.source_id,
                "evidence_id": supply_chain_evidence_id,
                "schema": supply_spec.schema,
                "sha256": "sha256:" + "b" * 64,
            },
        }
        write(
            valid_evidence / f"evidence/{supply_chain_evidence_id}.json",
            json.dumps(supply_normalized, sort_keys=True).encode() + b"\n",
        )
        artifact_digests = {
            artifact_id: "sha256:" + PACKAGE.sha256_file(valid_evidence / relative)
            for artifact_id, relative in artifact_paths.items()
            if artifact_id != "sigstore-bundle"
        }
        evidence_digests = {
            evidence_id: "sha256:"
            + PACKAGE.sha256_file(valid_evidence / f"evidence/{evidence_id}.json")
            for evidence_id in fixture_evidence_ids
        }
        ready_manifest_json = (
            json.dumps(ready_manifest, indent=2, sort_keys=True) + "\n"
        ).encode()
        write(
            valid_evidence / "release-manifest.json",
            json.dumps(
                {
                    "schema": PACKAGE.DETACHED_RELEASE_MANIFEST_SCHEMA,
                    "product": "0.1.0",
                    "source_version": "0.1.0",
                    "manifest": {
                        "source": "compatibility.toml",
                        "mirror": "compatibility.json",
                        "sha256": PACKAGE.sha256_bytes(ready_manifest_json),
                    },
                    "artifacts": artifact_paths,
                    "artifact_digests": artifact_digests,
                    "evidence": {
                        evidence_id: f"evidence/{evidence_id}.json"
                        for evidence_id in fixture_evidence_ids
                    },
                    "evidence_digests": evidence_digests,
                    "verification_material": {
                        "sigstore-bundle": {"path": "sigstore.bundle.json"}
                    },
                },
                indent=2,
                sort_keys=True,
            ).encode()
            + b"\n",
        )
        original_read_manifest = PACKAGE.read_manifest
        original_which = PACKAGE.shutil.which
        PACKAGE.read_manifest = lambda: (
            ready_manifest,
            manifest_toml,
            ready_manifest_json,
        )
        PACKAGE.shutil.which = lambda _name: None
        try:
            assert (
                PACKAGE.verify_release_directory(valid_evidence, structural_only=True)
                == 11
            )
        finally:
            PACKAGE.read_manifest = original_read_manifest
            PACKAGE.shutil.which = original_which

        detached_release_manifest = json.loads(
            (valid_evidence / "release-manifest.json").read_text()
        )
        self_referential_signature = json.loads(json.dumps(detached_release_manifest))
        self_referential_signature["artifact_digests"]["sigstore-bundle"] = (
            "sha256:"
            + PACKAGE.sha256_file(valid_evidence / artifact_paths["sigstore-bundle"])
        )
        write(
            valid_evidence / "release-manifest.json",
            json.dumps(self_referential_signature, indent=2, sort_keys=True).encode()
            + b"\n",
        )
        expect_package_error(
            lambda: PACKAGE.verify_release_directory(
                valid_evidence, structural_only=True
            ),
            "self-referential Sigstore bundle digest was accepted",
        )
        detached_release_manifest["artifact_digests"]["native-linux-x86_64-archive"] = (
            "sha256:" + "0" * 64
        )
        write(
            valid_evidence / "release-manifest.json",
            json.dumps(detached_release_manifest, indent=2, sort_keys=True).encode()
            + b"\n",
        )
        expect_package_error(
            lambda: PACKAGE.verify_release_directory(
                valid_evidence, structural_only=True
            ),
            "detached release manifest digest mismatch was accepted",
        )

        archive_entries = PACKAGE.archive_entries(first)
        root_name = "worldstream-0.1.0-linux-x86_64"
        relative_entries = [
            (name.removeprefix(root_name + "/"), content)
            for name, content in archive_entries.items()
        ]
        tampered_metadata = json.loads(dict(relative_entries)["metadata/release.json"])
        tampered_metadata["files"].append("fake/payload")
        tampered_entries = [
            (
                relative,
                (
                    json.dumps(tampered_metadata, indent=2, sort_keys=True) + "\n"
                ).encode()
                if relative == "metadata/release.json"
                else content,
            )
            for relative, content in relative_entries
        ]
        tampered = root / "tampered" / "worldstream-0.1.0-linux-x86_64.tar.gz"
        PACKAGE.write_tar_gz(tampered, root_name, tampered_entries, 0)
        expect_package_error(
            lambda: PACKAGE.verify_archive(tampered),
            "non-canonical release metadata was accepted",
        )
        tampered_identity_entries = [
            (
                relative,
                b"{}" if relative == PACKAGE.CLIENT_IDENTITY_SDK_PATH else content,
            )
            for relative, content in relative_entries
            if relative != "checksums.sha256"
        ]
        tampered_identity_entries.append(
            ("checksums.sha256", PACKAGE.checksums_file(tampered_identity_entries))
        )
        tampered_identity = (
            root / "tampered-identity" / "worldstream-0.1.0-linux-x86_64.tar.gz"
        )
        PACKAGE.write_tar_gz(tampered_identity, root_name, tampered_identity_entries, 0)
        expect_package_error(
            lambda: PACKAGE.verify_archive(tampered_identity),
            "archive client identity drift was accepted",
        )

        windows_inputs = {group: list(values) for group, values in inputs.items()}
        windows_inputs["bin"] = []
        for name, content in (
            ("worldstreamd.exe", b"windows daemon fixture"),
            ("worldstreamctl.exe", b"windows ctl fixture"),
        ):
            source = root / name
            write(source, content)
            windows_inputs["bin"].append((source, f"bin/{name}"))
        windows_files = PACKAGE.collect_package_files(
            PACKAGE.TARGETS["windows-x64"],
            "0.1.0",
            manifest_toml,
            manifest_json,
            windows_inputs,
            0,
            require_clean_checkout=False,
        )
        first_zip = root / "first-zip" / "worldstream-0.1.0-windows-x64.zip"
        second_zip = root / "second-zip" / "worldstream-0.1.0-windows-x64.zip"
        PACKAGE.write_zip(first_zip, "worldstream-0.1.0-windows-x64", windows_files, 0)
        PACKAGE.write_zip(second_zip, "worldstream-0.1.0-windows-x64", windows_files, 0)
        assert first_zip.read_bytes() == second_zip.read_bytes(), (
            "ZIP archive bytes are not reproducible"
        )
        PACKAGE.verify_archive(first_zip)

        # Source-only profile is deterministic and contains no native runtime
        # or signing claim.
        write(
            root / "Cargo.toml",
            b"[workspace]\nmembers = []\n[workspace.package]\nversion = '0.1.0'\n",
        )
        write(root / "Cargo.lock", b"# fixture lock\n")
        write(root / "compatibility.toml", manifest_toml)
        write(root / "compatibility.json", manifest_json)
        for generated in (
            ".hypothesis/state.bin",
            ".pnpm-store/cache.bin",
            "release-inputs/evidence.json",
            "reports/gate.json",
            "artifacts/result.bin",
            "coverage/index.html",
        ):
            write(root / generated, b"generated runner state")
        source_inputs = PACKAGE.required_inputs(
            PACKAGE.TARGETS["source"],
            root,
            root,
            root,
            root,
            root,
            source_dir=root,
        )
        source_destinations = {
            destination for _, destination in source_inputs["source"]
        }
        assert not any(
            destination.endswith(
                (
                    ".hypothesis/state.bin",
                    ".pnpm-store/cache.bin",
                    "release-inputs/evidence.json",
                    "reports/gate.json",
                    "artifacts/result.bin",
                    "coverage/index.html",
                )
            )
            for destination in source_destinations
        ), "source package included generated runner state"
        source_files = PACKAGE.collect_package_files(
            PACKAGE.TARGETS["source"],
            "0.1.0",
            manifest_toml,
            manifest_json,
            source_inputs,
            0,
            source_root=root,
            source_revision=PACKAGE.BUILD_IDENTITY.source_revision(
                Path(__file__).resolve().parents[1]
            ),
        )
        source_archive = root / "source" / "worldstream-0.1.0-source.tar.gz"
        PACKAGE.write_tar_gz(
            source_archive, "worldstream-0.1.0-source", source_files, 0
        )
        PACKAGE.verify_archive(source_archive)
        source_identity_files = dict(source_files)
        source_identity_files[PACKAGE.UI_SOURCE_IDENTITY_MODULE_PATH] = (
            b'import identity from "./compatibility_identity.json";\n'
            b"export const CLIENT_CONTRACT_IDENTITY = identity;\n"
        )
        expect_package_error(
            lambda: PACKAGE.validate_packaged_ui_consumed_identity(
                list(source_identity_files.items()), manifest, PACKAGE.TARGETS["source"]
            ),
            "source UI identity drift path was accepted with a valid public identity",
        )

        original_http_json = PACKAGE.http_json
        runtime_manifest = PACKAGE.read_manifest()[0]
        runtime_summary = PACKAGE.canonical_runtime_manifest_summary(runtime_manifest)
        runtime_source_revision = PACKAGE.BUILD_IDENTITY.source_revision(
            Path(__file__).resolve().parents[1]
        )
        sqlite_manifest = runtime_manifest["storage"]["sqlite"]
        runtime_responses = {
            "/healthz": (200, {"status": "ok"}),
            "/readyz": (503, {"error": {"code": "storage_not_initialized"}}),
            "/version": (
                200,
                {
                    "product_build": {
                        "product": "0.1.0",
                        "binary": "worldstreamd",
                        "build_version": "0.1.0",
                        "source_revision": runtime_source_revision,
                    },
                    "wire": "0.1",
                    "config": 1,
                    "storage_schema": 1,
                    "core_schema_version": "worldstream.core-room-state.v1",
                    "hash_suite": "blake3-canonical-json-v1",
                    "manifest": runtime_summary,
                    "engine": {
                        "profile": "sqlite-bundled",
                        "status": "verified",
                        "exact_identity": (
                            f"sqlite/{sqlite_manifest['version']}; "
                            f"source_id={sqlite_manifest['source_id']}"
                        ),
                    },
                },
            ),
        }
        PACKAGE.http_json = lambda _base, path, _timeout: runtime_responses[path]
        try:
            PACKAGE.runtime_probe(
                Namespace(
                    base_url="http://fixture",
                    version="0.1.0",
                    source_revision=runtime_source_revision,
                    timeout=1.0,
                )
            )
            drifted_summary = json.loads(json.dumps(runtime_summary))
            drifted_summary["pack_executors"][0]["revision_digest"] = "blake3:stale"
            runtime_responses["/version"][1]["manifest"] = drifted_summary
            expect_package_error(
                lambda: PACKAGE.runtime_probe(
                    Namespace(base_url="http://fixture", version="0.1.0", timeout=1.0)
                ),
                "runtime pack identity drift was accepted",
            )
            runtime_responses["/version"][1]["manifest"] = runtime_summary
            runtime_responses["/version"][1]["engine"]["exact_identity"] = (
                "sqlite/0.0.0; source_id=stale"
            )
            expect_package_error(
                lambda: PACKAGE.runtime_probe(
                    Namespace(base_url="http://fixture", version="0.1.0", timeout=1.0)
                ),
                "runtime engine identity drift was accepted",
            )
        finally:
            PACKAGE.http_json = original_http_json
        postgres_patch = runtime_manifest["storage"]["postgresql"][
            "release_verified_patches"
        ][0]
        postgres_major, postgres_minor = (
            int(component) for component in postgres_patch.split(".", 1)
        )
        PACKAGE.validate_runtime_engine_identity(
            {
                "profile": "postgres-primary",
                "status": "verified",
                "exact_identity": (
                    f"postgresql/{postgres_patch}; server_version_num="
                    f"{postgres_major * 10_000 + postgres_minor}"
                ),
            },
            runtime_manifest,
            "postgres-primary",
        )
        expect_package_error(
            lambda: PACKAGE.validate_runtime_engine_identity(
                {
                    "profile": "postgres-primary",
                    "status": "not_initialized",
                    "exact_identity": None,
                },
                runtime_manifest,
                "postgres-primary",
            ),
            "unverified PostgreSQL runtime identity was accepted",
        )

        oci_manifest = dict(manifest)
        oci_manifest["platforms"] = [
            {
                "id": "oci-linux-amd64",
                "target": "linux/amd64",
                "support": "release",
                "storage_profiles": ["sqlite-bundled", "postgres-primary"],
                "persistent_data_path": "/var/lib/worldstream",
                "read_only_root_compatible": True,
                "sqlite_overlay_allowed": False,
            }
        ]
        oci_manifest_json = (
            json.dumps(oci_manifest, indent=2, sort_keys=True) + "\n"
        ).encode()
        oci_sources = root / "oci-sources"
        write(
            oci_sources / "Dockerfile",
            (REPOSITORY_ROOT / "packaging/oci/Dockerfile").read_bytes(),
        )
        write(
            oci_sources / "entrypoint.sh",
            b"""#!/bin/sh\nset -eu\ndata_dir="${WORLDSTREAM__STORAGE__DATA_DIR:-/var/lib/worldstream}"\nprofile="${WORLDSTREAM__STORAGE__PROFILE:-sqlite-bundled}"\nif [ "$data_dir" != "/var/lib/worldstream" ]; then echo 'data directory must be /var/lib/worldstream' >&2; exit 78; fi\ncase "$(stat -f -c '%T' "$data_dir")" in ext4|xfs) ;; *) exit 78 ;; esac\n""",
        )
        write(
            oci_sources / "oci-metadata.json",
            json.dumps(
                {
                    "artifact": "worldstream-oci/v1",
                    "base_image": {
                        "digest": "",
                        "status": "must_be_pinned_by_release_evidence",
                    },
                    "profile": "oci-linux-amd64",
                    "image": {
                        "architecture": "amd64",
                        "os": "linux",
                        "healthcheck": (
                            "worldstreamctl --data-dir /var/lib/worldstream health"
                        ),
                    },
                }
            ).encode()
            + b"\n",
        )
        original_read_manifest = PACKAGE.read_manifest
        original_oci_files = PACKAGE.OCI_FILES
        PACKAGE.read_manifest = lambda: (
            oci_manifest,
            manifest_toml,
            oci_manifest_json,
        )
        PACKAGE.OCI_FILES = oci_sources
        try:
            oci_args = {
                "binary_dir": str(root),
                "ui_dir": str(root / "ui"),
                "sdk_dir": str(root / "sdk"),
                "examples_dir": str(root / "examples"),
                "licenses_dir": str(root / "licenses"),
                "base_image": (REPOSITORY_ROOT / "packaging/oci/base-image.txt")
                .read_text(encoding="utf-8")
                .strip(),
                "source_date_epoch": "0",
                "dry_run": False,
            }
            first_context = root / "oci-first"
            second_context = root / "oci-second"
            PACKAGE.oci_context(Namespace(output=str(first_context), **oci_args))
            PACKAGE.oci_context(Namespace(output=str(second_context), **oci_args))

            original_dockerfile = (first_context / "Dockerfile").read_bytes()
            write(first_context / "Dockerfile", original_dockerfile + b"\n# drift\n")
            expect_package_error(
                lambda: PACKAGE.verify_oci_context(first_context),
                "OCI context template drift was accepted",
            )
            write(first_context / "Dockerfile", original_dockerfile)
            os.utime(first_context / "Dockerfile", (0, 0))
            expect_package_error(
                lambda: PACKAGE.oci_context(
                    Namespace(
                        output=str(root / "oci-overlap"),
                        report=str(root / "oci-overlap" / "report.json"),
                        **oci_args,
                    )
                ),
                "OCI report path inside the context was accepted",
            )

            verified_oci_report = PACKAGE.verify_oci_context(first_context)
            assert verified_oci_report["schema"] == "worldstream/oci-context-report/v1"
            assert verified_oci_report["path"] == first_context.name
            assert verified_oci_report["path"] == verified_oci_report["artifact"]
            assert verified_oci_report["verified"] is True
            assert verified_oci_report["file_count"] > 0
            assert verified_oci_report["sha256"].startswith("sha256:")

            def context_snapshot(path: Path) -> dict[str, tuple[bytes, int, int]]:
                return {
                    file.relative_to(path).as_posix(): (
                        file.read_bytes(),
                        file.stat().st_mode & 0o777,
                        file.stat().st_mtime_ns,
                    )
                    for file in path.rglob("*")
                    if file.is_file()
                }

            assert context_snapshot(first_context) == context_snapshot(second_context)
            assert all(
                metadata[2] == 0
                for metadata in context_snapshot(first_context).values()
            ), "OCI context file mtimes are not SOURCE_DATE_EPOCH-normalized"
            assert all(
                file.stat().st_mtime_ns == 0 for file in first_context.rglob("*")
            ), "OCI context directory mtimes are not SOURCE_DATE_EPOCH-normalized"
        finally:
            PACKAGE.read_manifest = original_read_manifest
            PACKAGE.OCI_FILES = original_oci_files

        # Exercise the shell wrappers against a complete, release-valid
        # fixture workspace.  The wrappers must verify the produced archive or
        # context and emit exact digest/size accounting without creating a
        # release-manifest.json of their own.
        wrapper_root, wrapper_inputs = wrapper_workspace(
            root, manifest_toml, manifest_json
        )
        wrapper_output = wrapper_root / "dist"
        package_result = run_wrapper(
            wrapper_root,
            "package-release.sh",
            "--target",
            "source",
            "--source-dir",
            str(wrapper_root),
            "--output",
            str(wrapper_output),
        )
        assert package_result.returncode == 0, package_result.stderr
        archive_report = next(
            json.loads(line)
            for line in reversed(package_result.stdout.splitlines())
            if line.startswith("{")
        )
        archive_path = wrapper_output / archive_report["path"]
        assert archive_report["kind"] == "archive"
        assert archive_report["path"] == archive_report["artifact"]
        assert archive_report["size_bytes"] == archive_path.stat().st_size
        assert archive_report["sha256"] == "sha256:" + PACKAGE.sha256_file(archive_path)
        assert archive_report["identity"]["target"] == "source"
        assert archive_report["identity"]["version"] == "0.1.0"
        assert len(archive_report["identity"]["manifest_sha256"]) == 64
        assert (
            archive_report["identity"]["manifest_sha256"]
            == archive_report["identity"]["manifest_json_sha256"]
        )
        assert len(archive_report["identity"]["manifest_toml_sha256"]) == 64
        assert (
            archive_report["identity"]["manifest_toml_sha256"]
            != archive_report["identity"]["manifest_json_sha256"]
        )
        assert not (wrapper_output / "release-manifest.json").exists()

        report_path = wrapper_output / "source.report.json"
        reported_package = run_wrapper(
            wrapper_root,
            "package-release.sh",
            "--target",
            "source",
            "--source-dir",
            str(wrapper_root),
            "--output",
            str(wrapper_output / "reported"),
            "--report",
            str(report_path),
        )
        assert reported_package.returncode == 0, reported_package.stderr
        reported_archive = wrapper_output / "reported/worldstream-0.1.0-source.tar.gz"
        report = json.loads(report_path.read_text())
        assert report["schema"] == "worldstream/package-report/v1"
        assert report["path"] == reported_archive.name
        assert report["inventory"]["release_evidence"] is False
        verified_report = run_wrapper(
            wrapper_root,
            "verify-release.sh",
            str(reported_archive),
            "--report",
            str(report_path),
        )
        assert verified_report.returncode == 0, verified_report.stderr
        report_path.write_text(
            report_path.read_text().replace(report["sha256"], "sha256:" + "0" * 64)
        )
        rejected_report = run_wrapper(
            wrapper_root,
            "verify-release.sh",
            str(reported_archive),
            "--report",
            str(report_path),
        )
        assert rejected_report.returncode != 0

        missing_inputs = wrapper_root / "missing-inputs"
        shutil.copytree(wrapper_inputs, missing_inputs)
        (missing_inputs / "worldstreamctl").unlink()
        expect_package_error(
            lambda: PACKAGE.required_inputs(
                PACKAGE.TARGETS["linux-x86_64"],
                missing_inputs,
                missing_inputs / "ui",
                missing_inputs / "sdk",
                missing_inputs / "examples",
                wrapper_root / "missing-licenses",
                allow_missing=False,
            ),
            "missing native binaries/licenses were accepted",
        )

        dry_result = run_wrapper(
            wrapper_root,
            "package-release.sh",
            "--target",
            "linux-x86_64",
            "--binary-dir",
            str(missing_inputs),
            "--ui-dir",
            str(missing_inputs / "ui"),
            "--sdk-dir",
            str(missing_inputs / "sdk"),
            "--examples-dir",
            str(missing_inputs / "examples"),
            "--licenses-dir",
            str(wrapper_root / "missing-licenses"),
            "--output",
            str(wrapper_root / "dry-dist"),
            "--dry-run",
        )
        assert dry_result.returncode == 0, dry_result.stderr
        assert "blocked input" in dry_result.stdout
        assert not (wrapper_root / "dry-dist").exists()

        oci_arguments = (
            "--binary-dir",
            str(wrapper_inputs),
            "--ui-dir",
            str(wrapper_inputs / "ui"),
            "--sdk-dir",
            str(wrapper_inputs / "sdk"),
            "--examples-dir",
            str(wrapper_inputs / "examples"),
            "--licenses-dir",
            str(wrapper_inputs / "licenses"),
            "--base-image",
            (wrapper_root / "packaging/oci/base-image.txt")
            .read_text(encoding="utf-8")
            .strip(),
        )
        oci_first = run_wrapper(
            wrapper_root,
            "package-oci.sh",
            *oci_arguments,
            "--output",
            str(wrapper_root / "dist-oci-first"),
        )
        assert oci_first.returncode == 0, oci_first.stderr
        oci_first_report = next(
            json.loads(line)
            for line in reversed(oci_first.stdout.splitlines())
            if line.startswith("{")
        )
        oci_second = run_wrapper(
            wrapper_root,
            "package-oci.sh",
            *oci_arguments,
            "--output",
            str(wrapper_root / "dist-oci-second"),
        )
        assert oci_second.returncode == 0, oci_second.stderr
        oci_second_report = next(
            json.loads(line)
            for line in reversed(oci_second.stdout.splitlines())
            if line.startswith("{")
        )
        assert oci_first_report["kind"] == "oci-context"
        assert oci_first_report["size_bytes"] > 0
        assert oci_first_report["file_count"] > 0
        assert oci_first_report["size_bytes"] == oci_second_report["size_bytes"]
        assert oci_first_report["sha256"] == oci_second_report["sha256"]
        assert json.loads(
            (wrapper_root / "dist-oci-first/oci-metadata.json").read_text()
        )["runtime"] == {
            **json.loads(
                (wrapper_root / "dist-oci-second/oci-metadata.json").read_text()
            )["runtime"]
        }
        assert not (wrapper_root / "dist-oci-first/release-manifest.json").exists()

        oci_report_path = wrapper_root / "oci-context.report.json"
        oci_reported = run_wrapper(
            wrapper_root,
            "package-oci.sh",
            *oci_arguments,
            "--output",
            str(wrapper_root / "dist-oci-reported"),
            "--report",
            str(oci_report_path),
        )
        assert oci_reported.returncode == 0, oci_reported.stderr
        oci_report = json.loads(oci_report_path.read_text())
        assert oci_report["schema"] == "worldstream/oci-context-report/v1"
        assert oci_report["verified"] is True
        assert oci_report["inventory"]["schema"] == "worldstream/artifact-inventory/v1"
        assert oci_report["file_count"] == len(oci_report["inventory"]["files"])
        verified_context = run_wrapper(
            wrapper_root,
            "verify-release.sh",
            str(wrapper_root / "dist-oci-reported"),
            "--report",
            str(wrapper_root / "oci-context.verified.json"),
        )
        assert verified_context.returncode == 0, verified_context.stderr
        assert (
            json.loads((wrapper_root / "oci-context.verified.json").read_text())
            == oci_report
        )

        oci_missing_base = run_wrapper(
            wrapper_root,
            "package-oci.sh",
            "--binary-dir",
            str(wrapper_inputs),
            "--ui-dir",
            str(wrapper_inputs / "ui"),
            "--sdk-dir",
            str(wrapper_inputs / "sdk"),
            "--examples-dir",
            str(wrapper_inputs / "examples"),
            "--licenses-dir",
            str(wrapper_inputs / "licenses"),
            "--output",
            str(wrapper_root / "oci-missing-base"),
        )
        assert oci_missing_base.returncode != 0
        assert not (wrapper_root / "oci-missing-base").exists()

        assert (
            PACKAGE.sha256_bytes(manifest_json)
            == json.loads(
                PACKAGE.archive_entries(first)[
                    "worldstream-0.1.0-linux-x86_64/metadata/release.json"
                ]
            )["manifest"]["sha256"]
        )

        secret = root / "client.env"
        write(secret, b"WORLDSTREAM__STORAGE__POSTGRESQL__DSN=secret")
        try:
            PACKAGE.validate_source_file(secret, "client.env")
        except PACKAGE.PackageError:
            pass
        else:
            raise AssertionError("secret-like package input was accepted")

        for legitimate_name in (
            "scripts/verify-secret-absence.py",
            "tests/secret_scan.py",
            "docs/secret-management.md",
        ):
            legitimate = root / PurePosixPath(legitimate_name).name
            write(legitimate, b"legitimate source code or documentation")
            PACKAGE.validate_source_file(legitimate, legitimate_name)

        for secret_name in (
            ".env.production",
            "credentials.yaml",
            "secret.json",
            "id_ed25519",
            "client.key",
        ):
            secret_path = root / secret_name
            write(secret_path, b"sensitive fixture")
            expect_package_error(
                lambda path=secret_path, name=secret_name: PACKAGE.validate_source_file(
                    path, name
                ),
                f"secret-like package input was accepted: {secret_name}",
            )

        link = root / "linked.txt"
        link.symlink_to(root / "ui/index.html")
        try:
            PACKAGE.validate_source_file(link, "linked.txt")
        except PACKAGE.PackageError:
            pass
        else:
            raise AssertionError("symlink package input was accepted")

        for symlinked_binary in PACKAGE.TARGETS["linux-x86_64"].binary_names:
            binary_dir = root / f"symlink-bin-{symlinked_binary}"
            binary_dir.mkdir()
            real_binary = root / f"real-{symlinked_binary}"
            write(real_binary, b"native executable fixture")
            for binary in PACKAGE.TARGETS["linux-x86_64"].binary_names:
                destination = binary_dir / binary
                if binary == symlinked_binary:
                    destination.symlink_to(real_binary)
                else:
                    write(destination, b"native executable fixture")
            expect_package_error(
                lambda directory=binary_dir: PACKAGE.required_inputs(
                    PACKAGE.TARGETS["linux-x86_64"],
                    directory,
                    root / "ui",
                    root / "sdk",
                    root / "examples",
                    root / "licenses",
                ),
                f"symlinked required binary was accepted: {symlinked_binary}",
            )

        duplicate = root / "duplicate.tar.gz"
        with tarfile.open(duplicate, "w:gz") as archive:
            for content in (b"first", b"second"):
                member = tarfile.TarInfo("root/payload")
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
        try:
            PACKAGE.archive_entries(duplicate)
        except PACKAGE.PackageError:
            pass
        else:
            raise AssertionError("duplicate TAR member was accepted")

        quickstart = (
            Path(__file__).resolve().parents[1] / "scripts/macos-source-quickstart.sh"
        ).read_text(encoding="utf-8")
        for marker in (
            "sw_vers -productVersion",
            "macOS source quickstart requires macOS 15+",
            ".uv-version",
            "source_revision",
            "--storage-profile sqlite-bundled",
            "scripts/package.py probe",
            "no signed or notarized binary was produced",
        ):
            assert marker in quickstart, f"macOS source quickstart is missing {marker}"

        powershell_wrapper = (
            Path(__file__).resolve().parents[1] / "scripts/package-release.ps1"
        ).read_text(encoding="utf-8")
        assert "$ReportArgs.Add('report')" in powershell_wrapper
        assert '"$WorkspaceDir/scripts/package.py" @ReportArgs' in powershell_wrapper
        assert "ConvertTo-Json" not in powershell_wrapper
        assert "$null -eq $ReportPath" not in powershell_wrapper

        gates_path = Path(__file__).resolve().parents[1] / "scripts/gates.py"
        gates_spec = importlib.util.spec_from_file_location(
            "worldstream_gates", gates_path
        )
        if gates_spec is None or gates_spec.loader is None:
            raise RuntimeError("could not load gates.py")
        gates = importlib.util.module_from_spec(gates_spec)
        sys.modules["worldstream_gates"] = gates
        gates_spec.loader.exec_module(gates)
        mirror_path = root / "compatibility.json"
        mirror_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        original_mirror = gates.MIRROR_PATH
        gates.MIRROR_PATH = mirror_path
        try:
            runner = gates.GateRunner(strict=True, offline=True, ci=False)
            gates.manifest_gate(runner, manifest, release=True)
            statuses = {outcome.name: outcome.status for outcome in runner.outcomes}
            assert statuses["manifest-required-fields"] == "PASS"
            assert not any(outcome.status == "FAIL" for outcome in runner.outcomes)

            path_runner = gates.GateRunner(strict=True, offline=True, ci=False)
            assert (
                gates.safe_release_file(path_runner, root, "../outside", "path-safety")
                is None
            )
            assert any(
                outcome.name == "path-safety" and outcome.status == "FAIL"
                for outcome in path_runner.outcomes
            )
        finally:
            gates.MIRROR_PATH = original_mirror

    print("package archive smoke passed")


if __name__ == "__main__":
    main()
