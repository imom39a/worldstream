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
    assert len(module.pnpm_locked_packages(entries["pnpm-lock.yaml"])) == 95
    by_name = {package["name"]: package for package in packages}
    assert by_name["pydantic"]["downloadLocation"] == "https://pypi.org/simple"
    assert by_name["worldstream-sdk"]["downloadLocation"] == module.REPOSITORY


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
            "startedOn": "2026-08-22T00:00:00Z",
            "finishedOn": "2026-08-22T00:00:00Z",
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
