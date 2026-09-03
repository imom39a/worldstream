from __future__ import annotations

import hashlib
import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load(name: str, relative: str):
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / relative
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


INVENTORY = load("worldstream_test_release_inventory", "scripts/release_inventory.py")
SUBJECTS = load(
    "worldstream_test_cli_first_starter_subjects",
    "scripts/starter-release-subjects.py",
)
ASSEMBLER = load(
    "worldstream_test_cli_first_release_assembler",
    "scripts/release-evidence-assemble.py",
)
SUPPLY = load(
    "worldstream_test_cli_first_release_supply_chain",
    "scripts/release-supply-chain.py",
)
PACKAGE = load("worldstream_test_cli_first_package", "scripts/package.py")
STARTER = load(
    "worldstream_test_cli_first_starter_distribution",
    "scripts/starter-distribution.py",
)
IDENTITY = load(
    "worldstream_test_cli_first_build_identity",
    "scripts/release_build_identity.py",
)


def touch_subjects(root: Path, ids: tuple[str, ...]) -> list[str]:
    values = []
    for subject_id in ids:
        path = root / subject_id
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(subject_id.encode())
        values.append(f"{subject_id}={path}")
    return values


def release_manifest(profile, tmp_path: Path) -> dict:
    paths = {
        artifact_id: f"payload/{artifact_id}"
        for artifact_id in profile.release_artifact_ids
    }
    paths["sigstore-bundle"] = "sigstore.bundle.json"
    digests = {
        artifact_id: "sha256:" + "1" * 64
        for artifact_id in paths
        if artifact_id != "sigstore-bundle"
    }
    value = {
        "schema": profile.manifest_schema,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": "2" * 64,
        },
        "artifacts": paths,
        "artifact_digests": digests,
        "evidence": {"one": "evidence/one.json"},
        "evidence_digests": {"one": "sha256:" + "3" * 64},
        "verification_material": {"sigstore-bundle": {"path": "sigstore.bundle.json"}},
    }
    if profile.serialized_discriminator:
        value["release_inventory"] = profile.identity
    return value


def test_profiles_are_additive_and_remove_only_studio() -> None:
    legacy = INVENTORY.LEGACY
    cli = INVENTORY.CLI_FIRST
    assert len(legacy.portable_subject_artifact_ids) == 12
    assert len(cli.portable_subject_artifact_ids) == 11
    assert set(legacy.portable_subject_artifact_ids) - set(
        cli.portable_subject_artifact_ids
    ) == {"worldstream-studio"}
    assert "worldstream-participant-console" in cli.portable_subject_artifact_ids
    assert len(legacy.payload_artifact_ids) == 16
    assert len(cli.payload_artifact_ids) == 15
    assert len(legacy.release_artifact_ids) == 20
    assert len(cli.release_artifact_ids) == 19


def test_starter_subject_builder_uses_the_selected_closed_set(tmp_path: Path) -> None:
    cli = INVENTORY.CLI_FIRST
    values = touch_subjects(tmp_path, cli.portable_subject_artifact_ids)
    parsed = SUBJECTS.parse_subjects(values, cli.identity)
    assert set(parsed) == set(cli.portable_subject_artifact_ids)
    assert "worldstream-studio" not in SUBJECTS.output_names("0.1.0", cli.identity)

    studio = tmp_path / "worldstream-studio"
    studio.write_bytes(b"studio")
    with pytest.raises(SUBJECTS.SubjectError, match="unknown Starter subject id"):
        SUBJECTS.parse_subjects([*values, f"worldstream-studio={studio}"], cli.identity)


def test_detached_manifest_v3_binds_cli_inventory_without_changing_v2(
    tmp_path: Path,
) -> None:
    def paths(profile):
        result = {}
        for artifact_id in profile.payload_artifact_ids:
            path = tmp_path / artifact_id
            path.write_bytes(artifact_id.encode())
            result[artifact_id] = path
        for relative in ASSEMBLER.SIDECAR_PATHS.values():
            path = tmp_path / relative
            path.write_bytes(relative.encode())
        return result

    manifest = {"release_candidate": "0.1.0"}
    evidence_path = tmp_path / "evidence.json"
    evidence_path.write_bytes(b"evidence")
    legacy = ASSEMBLER.build_release_manifest(
        manifest,
        b"manifest",
        tmp_path,
        paths(INVENTORY.LEGACY),
        {"one": evidence_path},
    )
    assert legacy["schema"] == "worldstream/release-artifact-manifest/v2"
    assert "release_inventory" not in legacy
    assert "worldstream-studio" in legacy["artifacts"]

    cli = ASSEMBLER.build_release_manifest(
        manifest,
        b"manifest",
        tmp_path,
        paths(INVENTORY.CLI_FIRST),
        {"one": evidence_path},
        INVENTORY.CLI_FIRST.identity,
    )
    assert cli["schema"] == "worldstream/release-artifact-manifest/v3"
    assert cli["release_inventory"] == INVENTORY.CLI_FIRST.identity
    assert set(cli["artifacts"]) == set(INVENTORY.CLI_FIRST.release_artifact_ids)
    assert "worldstream-studio" not in cli["artifacts"]


def test_subject_inventory_v2_and_aggregation_v3_are_not_cross_substitutable(
    tmp_path: Path,
) -> None:
    profile = INVENTORY.CLI_FIRST
    payloads = {}
    for artifact_id in profile.payload_artifact_ids:
        path = tmp_path / f"payload-{artifact_id}"
        path.write_bytes(artifact_id.encode())
        payloads[artifact_id] = path
    sources = {}
    for index in range(17):
        path = tmp_path / f"source-{index}.json"
        path.write_bytes(str(index).encode())
        sources[f"source-{index}"] = path
    value = SUPPLY.inventory_value(
        "0.1.0", payloads, sources, tmp_path, profile.identity
    )
    assert value["schema"] == "worldstream/release-subject-inventory/v2"
    assert value["release_inventory"] == profile.identity
    assert len(value["subjects"]) == 32
    assert INVENTORY.identity_from_subject_inventory(value) is profile

    old_shape = dict(value)
    old_shape["schema"] = INVENTORY.LEGACY.subject_inventory_schema
    with pytest.raises(ValueError, match="legacy subject inventory"):
        INVENTORY.identity_from_subject_inventory(old_shape)

    aggregation = {
        "schema": profile.aggregation_schema,
        "release_inventory": profile.identity,
        "operation": "validate-and-copy",
        "product": "0.1.0",
        "source_revision": "1" * 40,
        "subject_count": 32,
        "component_graph_sha256": "sha256:" + "2" * 64,
        "evidence_producers": [{} for _ in range(17)],
        "payload_producers": [{} for _ in range(4)],
        "portable_subject_producers": [{} for _ in range(11)],
        "source_date_epoch": 0,
        "toolchains": {},
    }
    descriptor = IDENTITY.aggregation_byproduct(aggregation)
    assert descriptor["name"] == "worldstream-release-aggregation-v3.json"
    assert descriptor["mediaType"].endswith("release-aggregation.v3+json")
    old_aggregation = dict(aggregation)
    old_aggregation["schema"] = INVENTORY.LEGACY.aggregation_schema
    with pytest.raises(IDENTITY.IdentityError, match="legacy aggregation"):
        IDENTITY.aggregation_byproduct(old_aggregation)


def test_cli_native_archives_require_all_five_shipped_binaries() -> None:
    expected = INVENTORY.CLI_FIRST_NATIVE_BINARIES
    linux = PACKAGE.target_for_inventory(
        PACKAGE.TARGETS["linux-x86_64"], INVENTORY.CLI_FIRST.identity
    )
    windows = PACKAGE.target_for_inventory(
        PACKAGE.TARGETS["windows-x64"], INVENTORY.CLI_FIRST.identity
    )
    assert linux.binary_names == expected
    assert windows.binary_names == tuple(name + ".exe" for name in expected)
    assert PACKAGE.TARGETS["linux-x86_64"].binary_names == (
        "worldstreamd",
        "worldstreamctl",
    )


def test_starter_rejects_manifest_schema_and_subject_set_substitution(
    tmp_path: Path,
) -> None:
    cli = release_manifest(INVENTORY.CLI_FIRST, tmp_path)
    assert STARTER.validate_release_manifest(cli) is STARTER.INVENTORY.CLI_FIRST

    missing = {
        **cli,
        "artifacts": dict(cli["artifacts"]),
        "artifact_digests": dict(cli["artifact_digests"]),
    }
    missing["artifacts"].pop("worldstream-participant-console")
    missing["artifact_digests"].pop("worldstream-participant-console")
    with pytest.raises(STARTER.StarterError, match="versioned profile"):
        STARTER.validate_release_manifest(missing)

    cross_version = dict(cli)
    cross_version["schema"] = INVENTORY.LEGACY.manifest_schema
    with pytest.raises(STARTER.StarterError, match="legacy release manifest"):
        STARTER.validate_release_manifest(cross_version)

    # Historical v2 accepted a subset here; preserve that verifier behavior.
    legacy = release_manifest(INVENTORY.LEGACY, tmp_path)
    legacy["artifacts"] = {
        "sigstore-bundle": "sigstore.bundle.json",
        "source-archive": "source.tar.gz",
    }
    legacy["artifact_digests"] = {
        "source-archive": "sha256:" + hashlib.sha256(b"source").hexdigest()
    }
    assert STARTER.validate_release_manifest(legacy) is STARTER.INVENTORY.LEGACY


def test_v5_definition_is_content_addressed_and_active() -> None:
    content = (ROOT / IDENTITY.CLI_FIRST_BUILD_TYPE_PATH).read_bytes()
    assert b"defines the active CLI-first" in content
    assert hashlib.sha256(content).hexdigest() == IDENTITY.CLI_FIRST_BUILD_TYPE_SHA256
    assert IDENTITY.build_type_for(INVENTORY.CLI_FIRST.identity) == (
        "urn:worldstream:build-type:sha256:" + hashlib.sha256(content).hexdigest()
    )
    assert IDENTITY.build_type_for() == IDENTITY.BUILD_TYPE
