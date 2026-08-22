"""Security contract tests for fresh Linux package extraction."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import pathlib
import stat
import sys
import tarfile

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "release-package-extract.py"
PACKAGE_SMOKE = ROOT / "tests" / "package_smoke.py"


def load_module():
    spec = importlib.util.spec_from_file_location("release_package_extract", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_package_smoke():
    spec = importlib.util.spec_from_file_location(
        "release_package_extract_fixture_helpers", PACKAGE_SMOKE
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def add_file(
    archive: tarfile.TarFile, name: str, content: bytes, mode: int = 0o644
) -> None:
    member = tarfile.TarInfo(name)
    member.mode = mode
    member.size = len(content)
    archive.addfile(member, io.BytesIO(content))


def write_archive(
    path: pathlib.Path,
    daemon: bytes,
    manifest_toml: bytes,
    manifest_json: bytes,
    *,
    mutation: str | None = None,
) -> None:
    archive_root = "worldstream-0.1.0-linux-x86_64"
    with tarfile.open(path, "w:gz") as archive:
        add_file(archive, f"{archive_root}/bin/worldstreamd", daemon, 0o755)
        add_file(
            archive,
            f"{archive_root}/manifest/compatibility.toml",
            manifest_toml,
        )
        add_file(
            archive,
            f"{archive_root}/manifest/compatibility.json",
            manifest_json,
        )
        if mutation == "duplicate":
            add_file(archive, f"{archive_root}/bin/worldstreamd", daemon, 0o755)
        elif mutation == "second_daemon":
            add_file(archive, f"{archive_root}/other/bin/worldstreamd", daemon, 0o755)
        elif mutation == "non_executable":
            add_file(archive, f"{archive_root}/other/bin/worldstreamd", daemon, 0o644)
        elif mutation in {"absolute", "traversal", "backslash", "empty_segment"}:
            names = {
                "absolute": "/absolute/member",
                "traversal": "root/../member",
                "backslash": "root\\member",
                "empty_segment": "root//member",
            }
            add_file(archive, names[mutation], b"unsafe")
        elif mutation == "link":
            member = tarfile.TarInfo(f"{archive_root}/link")
            member.type = tarfile.SYMTYPE
            member.linkname = "../../outside"
            archive.addfile(member)
        elif mutation == "special":
            member = tarfile.TarInfo(f"{archive_root}/fifo")
            member.type = tarfile.FIFOTYPE
            archive.addfile(member)
        elif mutation == "oversized_control":
            add_file(
                archive,
                f"{archive_root}/metadata/profile.json",
                b"x" * 65,
            )
        elif mutation == "mixed_root":
            add_file(archive, "other-root/member", b"mixed")


def write_report(
    path: pathlib.Path,
    archive: pathlib.Path,
    manifest_toml: bytes,
    manifest_json: bytes,
) -> None:
    path.write_text(
        json.dumps(
            {
                "schema": "worldstream/package-report/v1",
                "kind": "archive",
                "artifact": archive.name,
                "path": archive.name,
                "sha256": "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest(),
                "size_bytes": archive.stat().st_size,
                "inventory": {
                    "archive_verified": True,
                    "release_evidence": False,
                    "manifest_source": "compatibility.toml",
                    "manifest_mirror": "compatibility.json",
                },
                "identity": {
                    "target": "linux-x86_64",
                    "version": "0.1.0",
                    "manifest_sha256": hashlib.sha256(manifest_json).hexdigest(),
                    "manifest_json_sha256": hashlib.sha256(manifest_json).hexdigest(),
                    "manifest_toml_sha256": hashlib.sha256(manifest_toml).hexdigest(),
                },
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


@pytest.fixture
def fixture(tmp_path: pathlib.Path):
    module = load_module()
    module.PACKAGE.verify_archive = lambda _path: None
    daemon = b"fresh packaged daemon\n"
    manifest_toml = b'manifest_revision = 1\nschema = "fixture/v1"\n'
    manifest_json = b'{\n  "manifest_revision": 1,\n  "schema": "fixture/v1"\n}\n'
    toml_path = tmp_path / "compatibility.toml"
    json_path = tmp_path / "compatibility.json"
    toml_path.write_bytes(manifest_toml)
    json_path.write_bytes(manifest_json)
    archive = tmp_path / "worldstream-0.1.0-linux-x86_64.tar.gz"
    report = tmp_path / "package-report.json"
    write_archive(archive, daemon, manifest_toml, manifest_json)
    write_report(report, archive, manifest_toml, manifest_json)
    (tmp_path / "extracted").mkdir()
    return {
        "module": module,
        "daemon": daemon,
        "toml": toml_path,
        "json": json_path,
        "toml_bytes": manifest_toml,
        "json_bytes": manifest_json,
        "archive": archive,
        "report": report,
        "output": tmp_path / "extracted" / "worldstreamd",
    }


def invoke(value):
    return value["module"].extract(
        value["archive"],
        value["report"],
        value["output"],
        value["toml"],
        value["json"],
    )


def test_extracts_exact_new_executable_and_returns_identity(fixture):
    result = invoke(fixture)
    assert result["status"] == "pass"
    assert result["target"] == "linux-x86_64"
    assert result["output"] == "worldstreamd"
    assert fixture["output"].read_bytes() == fixture["daemon"]
    assert stat.S_IMODE(fixture["output"].stat().st_mode) == 0o755
    assert result["binary_sha256"] == (
        "sha256:" + hashlib.sha256(fixture["daemon"]).hexdigest()
    )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("artifact", "other.tar.gz"),
        ("path", "/runner/absolute.tar.gz"),
        ("sha256", "sha256:" + "0" * 64),
        ("size_bytes", 1),
    ],
)
def test_report_must_exactly_bind_portable_artifact(fixture, field, value):
    report = json.loads(fixture["report"].read_text())
    report[field] = value
    fixture["report"].write_text(json.dumps(report) + "\n")
    with pytest.raises(fixture["module"].ExtractionError, match="exactly bind"):
        invoke(fixture)


@pytest.mark.parametrize(
    "field", ["manifest_sha256", "manifest_json_sha256", "manifest_toml_sha256"]
)
def test_report_must_bind_both_explicit_manifest_digests(fixture, field):
    report = json.loads(fixture["report"].read_text())
    report["identity"][field] = "0" * 64
    fixture["report"].write_text(json.dumps(report) + "\n")
    with pytest.raises(fixture["module"].ExtractionError, match="both exact"):
        invoke(fixture)


@pytest.mark.parametrize(
    "mutation",
    [
        "duplicate",
        "absolute",
        "traversal",
        "backslash",
        "empty_segment",
        "link",
        "special",
    ],
)
def test_rejects_duplicate_unsafe_link_and_special_members(fixture, mutation):
    write_archive(
        fixture["archive"],
        fixture["daemon"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
        mutation=mutation,
    )
    write_report(
        fixture["report"],
        fixture["archive"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
    )
    with pytest.raises(fixture["module"].ExtractionError):
        invoke(fixture)


def test_requires_exactly_one_executable_daemon(fixture):
    write_archive(
        fixture["archive"],
        fixture["daemon"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
        mutation="second_daemon",
    )
    write_report(
        fixture["report"],
        fixture["archive"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
    )
    with pytest.raises(fixture["module"].ExtractionError, match="exactly one"):
        invoke(fixture)


def test_root_manifest_bytes_are_not_trusted_by_digest_alone(fixture):
    fixture["toml"].write_bytes(fixture["toml_bytes"] + b"# stale\n")
    with pytest.raises(fixture["module"].ExtractionError, match="both exact"):
        invoke(fixture)


def test_output_must_be_new_and_distinct_from_every_input(fixture):
    fixture["output"].write_bytes(b"existing")
    with pytest.raises(fixture["module"].ExtractionError, match="output must be new"):
        invoke(fixture)
    assert fixture["output"].read_bytes() == b"existing"

    fixture["output"].unlink()
    fixture["output"] = fixture["archive"]
    with pytest.raises(fixture["module"].ExtractionError, match="outside"):
        invoke(fixture)


def test_rejects_mixed_archive_roots(fixture):
    write_archive(
        fixture["archive"],
        fixture["daemon"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
        mutation="mixed_root",
    )
    write_report(
        fixture["report"],
        fixture["archive"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
    )
    with pytest.raises(
        fixture["module"].ExtractionError, match="canonical package root"
    ):
        invoke(fixture)


def test_oversized_control_member_is_rejected_before_reading(
    fixture, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setattr(fixture["module"], "MAX_ARCHIVE_CONTROL_FILE_BYTES", 64)
    write_archive(
        fixture["archive"],
        fixture["daemon"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
        mutation="oversized_control",
    )
    write_report(
        fixture["report"],
        fixture["archive"],
        fixture["toml_bytes"],
        fixture["json_bytes"],
    )
    with pytest.raises(fixture["module"].ExtractionError, match="control member"):
        invoke(fixture)


def test_symlinked_output_ancestor_is_rejected(fixture):
    real_parent = fixture["output"].parents[1] / "real-parent"
    (real_parent / "child").mkdir(parents=True)
    linked_parent = fixture["output"].parents[1] / "linked-parent"
    linked_parent.symlink_to(real_parent, target_is_directory=True)
    fixture["output"] = linked_parent / "child" / "worldstreamd"
    with pytest.raises(fixture["module"].ExtractionError, match="all ancestors"):
        invoke(fixture)


@pytest.fixture
def canonical_fixture(tmp_path: pathlib.Path):
    module = load_module()
    helpers = load_package_smoke()
    manifest, manifest_toml, manifest_json = helpers.PACKAGE.read_manifest()
    version = manifest["contracts"]["product"]
    inputs = helpers.fixture_inputs(tmp_path / "inputs")
    (tmp_path / "inputs/examples/heist/parity_fixture.json").write_bytes(
        (ROOT / "examples/heist/parity_fixture.json").read_bytes()
    )
    helpers.add_fixture_client_identities(tmp_path / "inputs", inputs, manifest)
    files = helpers.PACKAGE.collect_package_files(
        helpers.PACKAGE.TARGETS["linux-x86_64"],
        version,
        manifest_toml,
        manifest_json,
        inputs,
        0,
        require_clean_checkout=False,
    )
    archive_root = f"worldstream-{version}-linux-x86_64"
    archive = tmp_path / f"{archive_root}.tar.gz"
    helpers.PACKAGE.write_tar_gz(archive, archive_root, files, 0)
    report = tmp_path / "package-report.json"
    report.write_text(
        json.dumps(helpers.PACKAGE.archive_report(archive), sort_keys=True) + "\n",
        encoding="utf-8",
    )
    output = tmp_path / "extracted" / "worldstreamd"
    output.parent.mkdir()
    return {
        "module": module,
        "helpers": helpers,
        "manifest_toml": ROOT / "compatibility.toml",
        "manifest_json": ROOT / "compatibility.json",
        "archive": archive,
        "archive_root": archive_root,
        "report": report,
        "output": output,
    }


def canonical_invoke(value):
    return value["module"].extract(
        value["archive"],
        value["report"],
        value["output"],
        value["manifest_toml"],
        value["manifest_json"],
    )


def rewrite_canonical_archive(value, mutator) -> None:
    package = value["helpers"].PACKAGE
    entries = package.archive_entries(value["archive"])
    relative = [
        (name.removeprefix(value["archive_root"] + "/"), content)
        for name, content in entries.items()
    ]
    rewritten = mutator(relative)
    package.write_tar_gz(value["archive"], value["archive_root"], rewritten, 0)
    value["report"].write_text(
        json.dumps(package.archive_report(value["archive"]), sort_keys=True) + "\n",
        encoding="utf-8",
    )


def test_canonical_package_verifier_accepts_real_package_layout(canonical_fixture):
    result = canonical_invoke(canonical_fixture)
    assert result["status"] == "pass"


def test_forged_archive_verified_cannot_hide_checksum_drift(canonical_fixture):
    def corrupt_checksums(entries):
        return [
            (path, b"0" * len(content) if path == "checksums.sha256" else content)
            for path, content in entries
        ]

    rewrite_canonical_archive(canonical_fixture, corrupt_checksums)
    report = json.loads(canonical_fixture["report"].read_text())
    assert report["inventory"]["archive_verified"] is True
    with pytest.raises(
        canonical_fixture["module"].ExtractionError, match="canonical package"
    ):
        canonical_invoke(canonical_fixture)


def test_canonical_metadata_drift_is_rejected(canonical_fixture):
    package = canonical_fixture["helpers"].PACKAGE

    def corrupt_metadata(entries):
        result = []
        for path, content in entries:
            if path == "metadata/release.json":
                metadata = json.loads(content)
                metadata["source_date_epoch"] = 1
                content = (
                    json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True)
                    + "\n"
                ).encode()
            if path != "checksums.sha256":
                result.append((path, content))
        result.append(("checksums.sha256", package.checksums_file(result)))
        return result

    rewrite_canonical_archive(canonical_fixture, corrupt_metadata)
    with pytest.raises(
        canonical_fixture["module"].ExtractionError, match="canonical package"
    ):
        canonical_invoke(canonical_fixture)
