from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    name = "worldstream_test_starter_release_subjects"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / "scripts/starter-release-subjects.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


SUBJECTS = load_module()


def subject_inputs(root: Path) -> list[str]:
    values: list[str] = []
    compatibility = SUBJECTS.strict_json(
        (ROOT / "compatibility.json").read_bytes(), "compatibility manifest"
    )
    official = next(
        row
        for row in compatibility["activity_pack_bundles"]
        if row["pack_id"] == "worldstream.negotiate"
        and row.get("required_for_release") is True
    )
    for subject_id in SUBJECTS.SUBJECT_IDS:
        if subject_id == "worldstream-negotiate-bundle":
            source = ROOT / official["path"]
        else:
            source = root / subject_id
            source.mkdir(parents=True)
            (source / "README.txt").write_text(
                f"fixture for {subject_id}\n", encoding="utf-8"
            )
        values.append(f"{subject_id}={source}")
    return values


def build(root: Path, inputs: list[str]) -> Path:
    output = root / "payload"
    SUBJECTS.build(
        SimpleNamespace(
            subject=inputs,
            version="0.1.0",
            output_dir=output,
            compatibility_json=ROOT / "compatibility.json",
        )
    )
    return output


def test_builds_closed_deterministic_subject_inventory(tmp_path: Path) -> None:
    first_inputs = subject_inputs(tmp_path / "first-inputs")
    first = build(tmp_path / "first", first_inputs)
    second = build(tmp_path / "second", first_inputs)

    first_bytes = {path.name: path.read_bytes() for path in first.iterdir()}
    second_bytes = {path.name: path.read_bytes() for path in second.iterdir()}
    assert first_bytes == second_bytes
    report = SUBJECTS.verify_payload_dir(first, "0.1.0", ROOT / "compatibility.json")
    assert report["status"] == "passed"
    assert set(report["subjects"]) == set(SUBJECTS.SUBJECT_IDS)


def test_rejects_symlink_secret_and_extra_subject_material(tmp_path: Path) -> None:
    source = tmp_path / "input"
    source.mkdir()
    target = source / "payload.txt"
    target.write_text("payload\n", encoding="utf-8")
    (source / "alias.txt").symlink_to(target)
    with pytest.raises(SUBJECTS.SubjectError, match="symlink"):
        SUBJECTS.input_files(source, "worldstream-documentation")

    secret = tmp_path / "secrets"
    secret.mkdir()
    (secret / "token.pem").write_text("not a real key\n", encoding="utf-8")
    with pytest.raises(SUBJECTS.SubjectError, match="credential|secret"):
        SUBJECTS.input_files(secret, "worldstream-documentation")

    inputs = subject_inputs(tmp_path / "closed-inputs")
    output = build(tmp_path / "closed", inputs)
    (output / "unexpected.txt").write_text("extra\n", encoding="utf-8")
    with pytest.raises(SUBJECTS.SubjectError, match="not closed"):
        SUBJECTS.verify_payload_dir(output, "0.1.0", ROOT / "compatibility.json")


def test_rejects_substituted_official_bundle_and_noncanonical_wrapper(
    tmp_path: Path,
) -> None:
    inputs = subject_inputs(tmp_path / "inputs")
    output = build(tmp_path / "built", inputs)
    names = SUBJECTS.output_names("0.1.0")

    bundle = output / names["worldstream-negotiate-bundle"]
    bundle.write_bytes(bundle.read_bytes() + b"substitution")
    with pytest.raises(
        SUBJECTS.SubjectError, match="bundle|archive|identity|digest|ZIP"
    ):
        SUBJECTS.verify_payload_dir(output, "0.1.0", ROOT / "compatibility.json")

    output = build(tmp_path / "rebuilt", inputs)
    wrapped = output / names["worldstream-documentation"]
    content = bytearray(wrapped.read_bytes())
    content[4] = 1
    wrapped.write_bytes(content)
    with pytest.raises(SUBJECTS.SubjectError, match="deterministic gzip"):
        SUBJECTS.verify_payload_dir(output, "0.1.0", ROOT / "compatibility.json")
