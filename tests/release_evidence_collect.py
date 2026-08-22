"""Focused fail-closed tests for detached release evidence collection."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
COLLECT = ROOT / "scripts/release-evidence-collect.py"
ASSEMBLE = ROOT / "scripts/release-evidence-assemble.py"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture()
def collector(tmp_path: Path):
    module = load_module("release_evidence_collect", COLLECT)
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_json = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", manifest_toml)
    shutil.copy2(ROOT / "compatibility.json", manifest_json)
    return module, manifest_toml, manifest_json, tmp_path


def valid_sources(module, manifest_toml: Path, tmp_path: Path) -> dict[str, Path]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    manifest = module.load_manifest(
        manifest_toml, manifest_toml.with_name("compatibility.json")
    )
    sources: dict[str, Path] = {}
    for spec in module.SOURCE_SPECS:
        path = tmp_path / f"{spec.source_id}.json"
        path.write_text(
            json.dumps(
                {
                    "schema": spec.schema,
                    "source_id": spec.source_id,
                    "evidence_id": spec.evidence_id,
                    "status": "passed",
                    "release_evidence": True,
                    "fail_closed": False,
                    "version": manifest["release_candidate"],
                    "platform": spec.platform,
                    "contract": manifest["contracts"],
                    "checks": {check: True for check in spec.checks},
                    "details": {
                        "producer_id": module.EXPECTED_PRODUCER_IDS[spec.source_id],
                        "phase": module.PRE_SIGN_PHASE,
                        "outcomes": {
                            check: {
                                "status": "passed",
                                "observations": [{"kind": "fixture", "value": check}],
                            }
                            for check in spec.checks
                        },
                        "artifacts": {
                            binding_id: {
                                "sha256": "sha256:" + "a" * 64,
                                "size_bytes": 1,
                            }
                            for binding_id in module.REQUIRED_ARTIFACT_BINDINGS[
                                spec.source_id
                            ]
                        },
                    },
                },
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        sources[spec.source_id] = path
    return sources


def test_collects_exact_manifest_ids_and_cites_hashed_sources(collector):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    output = tmp_path / "evidence"

    evidence_ids = module.collect(output, sources, manifest_toml, manifest_json)

    assert len(evidence_ids) == module.REQUIRED_RELEASE_EVIDENCE_COUNT
    assert {path.name.removesuffix(".json") for path in output.iterdir()} == set(
        evidence_ids
    )
    for source_id, source_path in sources.items():
        spec = module.SOURCE_BY_ID[source_id]
        value = json.loads(
            (output / f"{spec.evidence_id}.json").read_text(encoding="utf-8")
        )
        assert value["schema"] == module.NORMALIZED_SCHEMA
        assert value["evidence_id"] == spec.evidence_id
        assert value["release_gate"] is True
        assert value["release_evidence"] is True
        assert value["status"] == "passed"
        assert value["producer_details"]["phase"] == module.PRE_SIGN_PHASE
        assert set(value["producer_details"]["artifacts"]) == set(
            module.REQUIRED_ARTIFACT_BINDINGS[source_id]
        )
        assert set(value["source_report"]) == {
            "source_id",
            "evidence_id",
            "schema",
            "sha256",
        }
        assert "path" not in value["source_report"]
        assert value["source_report"]["source_id"] == source_id
        expected_hash = hashlib.sha256(source_path.read_bytes()).hexdigest()
        assert value["source_report"]["sha256"] == f"sha256:{expected_hash}"

    manifest = module.load_manifest(manifest_toml, manifest_json)
    assembler = load_module("release_evidence_assemble_for_collect", ASSEMBLE)
    assembled_ids = assembler.validate_evidence_inputs(
        output,
        evidence_ids,
        manifest["release_candidate"],
        manifest["contracts"],
    )
    assert set(assembled_ids) == set(evidence_ids)


def test_shared_release_json_parser_rejects_duplicate_security_keys(collector):
    module, _manifest_toml, _manifest_json, _tmp_path = collector

    with pytest.raises(module.CollectionError, match="duplicate key 'status'"):
        module.strict_json_object(
            b'{"status":"passed","status":"failed"}', "source report"
        )
    with pytest.raises(module.CollectionError, match="duplicate key 'subject'"):
        module.strict_json_object(
            b'{"predicate":{"subject":[],"subject":[{"name":"hidden"}]}}',
            "signed provenance",
        )


def test_collector_bounds_control_file_reads(collector, monkeypatch):
    module, _manifest_toml, _manifest_json, tmp_path = collector
    document = b'{"value":"bounded"}'
    monkeypatch.setattr(module.BUILD_IDENTITY, "MAX_RELEASE_JSON_BYTES", len(document))
    path = tmp_path / "control.json"
    path.write_bytes(document)

    assert module.bounded_regular_bytes(path, "control JSON") == document
    path.write_bytes(document + b" ")
    with pytest.raises(module.CollectionError, match="too large"):
        module.bounded_regular_bytes(path, "control JSON")


def test_identical_source_bytes_in_different_roots_have_identical_output(collector):
    module, manifest_toml, manifest_json, tmp_path = collector
    first_sources = valid_sources(module, manifest_toml, tmp_path / "first" / "sources")
    second_root = tmp_path / "second"
    second_manifest_toml = second_root / "compatibility.toml"
    second_manifest_json = second_root / "compatibility.json"
    second_root.mkdir()
    shutil.copy2(manifest_toml, second_manifest_toml)
    shutil.copy2(manifest_json, second_manifest_json)
    second_sources = valid_sources(
        module, second_manifest_toml, second_root / "sources"
    )

    first_output = tmp_path / "first" / "evidence"
    second_output = second_root / "evidence"
    module.collect(first_output, first_sources, manifest_toml, manifest_json)
    module.collect(
        second_output, second_sources, second_manifest_toml, second_manifest_json
    )

    for spec in module.SOURCE_SPECS:
        assert (first_output / f"{spec.evidence_id}.json").read_bytes() == (
            second_output / f"{spec.evidence_id}.json"
        ).read_bytes()


def test_missing_source_fails_before_writing(collector):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    sources.pop("failure-soak")
    output = tmp_path / "evidence"

    with pytest.raises(module.CollectionError, match="exactly the mapped source IDs"):
        module.collect(output, sources, manifest_toml, manifest_json)
    assert not output.exists()


def test_duplicate_source_path_fails_closed(collector):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    sources["failure-soak"] = sources["manifest-contract"]

    with pytest.raises(module.CollectionError, match="duplicate source report"):
        module.collect(tmp_path / "evidence", sources, manifest_toml, manifest_json)


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("release_evidence", False, "release_evidence=true"),
        ("status", "unavailable", "not passed"),
        ("status", "incomplete", "not passed"),
        ("schema", "worldstream/package-report/v1", "wrong schema"),
        ("platform", "native-windows-x64", "wrong platform"),
        ("version", "9.9.9", "wrong version"),
    ],
)
def test_unavailable_incomplete_and_identity_mismatches_fail_closed(
    collector, field, value, message
):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    target = sources["native-linux"]
    report = json.loads(target.read_text(encoding="utf-8"))
    report[field] = value
    target.write_text(json.dumps(report) + "\n", encoding="utf-8")

    with pytest.raises(module.CollectionError, match=message):
        module.collect(tmp_path / "evidence", sources, manifest_toml, manifest_json)


def test_file_presence_or_exit_code_cannot_create_pass_evidence(collector):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    target = sources["native-linux"]
    target.write_text('{"exit_code": 0}\n', encoding="utf-8")
    output = tmp_path / "evidence"

    with pytest.raises(module.CollectionError, match="wrong fields"):
        module.collect(output, sources, manifest_toml, manifest_json)
    assert not output.exists()


def test_exact_contract_fields_are_required_and_no_extra_contract_is_accepted(
    collector,
):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    target = sources["manifest-contract"]
    report = json.loads(target.read_text(encoding="utf-8"))
    report["contract"] = dict(report["contract"])
    report["contract"]["unexpected"] = True
    target.write_text(json.dumps(report) + "\n", encoding="utf-8")

    with pytest.raises(module.CollectionError, match="compatibility contract"):
        module.collect(tmp_path / "evidence", sources, manifest_toml, manifest_json)


@pytest.mark.parametrize("bad_status", [[], {}, 1])
def test_unhashable_source_status_fails_closed(collector, bad_status):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    target = sources["native-linux"]
    report = json.loads(target.read_text(encoding="utf-8"))
    report["status"] = bad_status
    target.write_text(json.dumps(report) + "\n", encoding="utf-8")

    with pytest.raises(module.CollectionError, match="status must be a string"):
        module.collect(tmp_path / "evidence", sources, manifest_toml, manifest_json)


@pytest.mark.parametrize("bad_status", [[], {}, 1])
def test_unhashable_source_outcome_status_fails_closed(collector, bad_status):
    module, manifest_toml, manifest_json, tmp_path = collector
    sources = valid_sources(module, manifest_toml, tmp_path / "sources")
    target = sources["native-linux"]
    report = json.loads(target.read_text(encoding="utf-8"))
    report["details"]["outcomes"]["archive_identity"]["status"] = bad_status
    target.write_text(json.dumps(report) + "\n", encoding="utf-8")

    with pytest.raises(module.CollectionError, match="status must be a string"):
        module.collect(tmp_path / "evidence", sources, manifest_toml, manifest_json)
