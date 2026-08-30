from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import json
import sys
import tarfile
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    name = "worldstream_starter_distribution"
    spec = importlib.util.spec_from_file_location(
        name, ROOT / "scripts/starter-distribution.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


STARTER = load_module()


def canonical(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def compact(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode()


def ustar(files: dict[str, bytes]) -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, content in sorted(files.items()):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            archive.addfile(info, io.BytesIO(content))
    return output.getvalue()


def pack_bytes(pack_id: str = "worldstream.negotiate") -> tuple[bytes, dict[str, str]]:
    files = {
        "codec-bundle.json": compact({"codec": "v1"}),
        "conformance.json": compact({"status": "passed"}),
        "dependency-lock.json": compact({"dependencies": []}),
        "descriptor.json": compact(
            {"explanatory_version": "0.1.0", "pack_id": pack_id}
        ),
        "executor.component.wasm": b"\x00asm\x0d\x00\x01\x00",
        "golden-corpus.json": compact({"cases": []}),
        "revision-lock.json": compact({"lock": pack_id}),
        "schemas.json": compact({"schemas": []}),
    }
    revision = STARTER.blake3_reference(files["revision-lock.json"])
    bundle_manifest = {
        "bundle_format_id": STARTER.PACK_BUNDLE_SCHEMA,
        "canonical_codec": "worldstream/canonical-json/v1",
        "execution_profile_id": "worldstream/component-deterministic/v1",
        "host_contract": "worldstream/activity-pack/v1",
        "members": [
            {
                "blake3": STARTER.blake3_reference(content),
                "name": name,
                "size": len(content),
            }
            for name, content in sorted(files.items())
        ],
        "revision_digest": revision,
        "static_members": [],
    }
    content = ustar({"bundle-manifest.json": compact(bundle_manifest), **files})
    return content, {
        "pack_id": pack_id,
        "explanatory_version": "0.1.0",
        "revision_digest": revision,
        "bundle_digest": STARTER.blake3_reference(content),
    }


def evidence(pack: dict[str, str]) -> bytes:
    return canonical(
        {
            "bundle": {
                "bundle_digest": pack["bundle_digest"],
                "revision_digest": pack["revision_digest"],
            },
            "evidence_id": "worldstream/negotiate-official-pack-evidence/v1",
            "pack_id": pack["pack_id"],
            "production_proof": {
                "complete": {
                    "bundle_digest": pack["bundle_digest"],
                    "proof_type": "complete",
                    "revision_digest": pack["revision_digest"],
                    "status": "passed",
                }
            },
            "result": "passed",
            "version": pack["explanatory_version"],
        }
    )


def subject_filename(role: str) -> str:
    return {
        "negotiate-bundle": "worldstream-negotiate.wspack",
        "negotiate-evidence": "negotiate-evidence.json",
    }.get(role, f"{role}.tar.gz")


def fixture(tmp_path: Path, *, custom: bool = False) -> tuple[Path, dict[str, Any]]:
    release_dir = tmp_path / "release"
    release_dir.mkdir(parents=True)
    official_bundle, official_pack = pack_bytes()
    official_evidence = evidence(official_pack)
    payloads: dict[str, bytes] = {}
    payload_paths: dict[str, str] = {}
    subjects: list[dict[str, Any]] = []
    roles = set(STARTER.BASE_ROLES)
    if not custom:
        roles |= {"negotiate-bundle", "negotiate-evidence"}
    for role in sorted(roles):
        subject_id = role
        expected_binding = STARTER.expected_release_binding(role, "native-linux-x86_64")
        assert expected_binding is not None
        inventory_kind, release_id = expected_binding
        filename = subject_filename(role)
        release_path = f"subjects/{filename}"
        if role == "negotiate-bundle":
            content = official_bundle
        elif role == "negotiate-evidence":
            content = official_evidence
        else:
            content = f"exact {role} release bytes\n".encode()
        path = release_dir / release_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        payloads[release_id] = content
        payload_paths[release_id] = release_path
        subjects.append(
            {
                "binding": {
                    "id": release_id,
                    "inventory": inventory_kind,
                    "kind": "release",
                },
                "id": subject_id,
                "license_expression": "Apache-2.0",
                "media_type": "application/octet-stream",
                "role": role,
            }
        )

    compatibility = {
        "activity_pack_bundles": [
            {
                **official_pack,
                "bundle_format_id": STARTER.PACK_BUNDLE_SCHEMA,
                "required_for_release": True,
                "status": "resolved",
            }
        ],
        "contracts": {"product": "0.1.0"},
        "manifest_kind": "release",
        "release_candidate": "0.1.0",
        "release_ready": True,
        "schema": "worldstream/storage-compatibility-manifest/v1",
        "unresolved_required_fields": [],
    }
    compatibility_path = tmp_path / "compatibility.json"
    compatibility_path.write_bytes(canonical(compatibility))

    artifacts = {
        release_id: payload_paths[release_id]
        for release_id in payloads
        if release_id != "worldstream-negotiate-evidence"
    }
    artifacts["sigstore-bundle"] = "sigstore.bundle.json"
    artifact_digests = {
        subject_id: "sha256:" + hashlib.sha256(payloads[subject_id]).hexdigest()
        for subject_id in artifacts
        if subject_id != "sigstore-bundle"
    }
    qualification = b"exact release qualification evidence\n"
    qualification_path = release_dir / "evidence/release-qualification.json"
    qualification_path.parent.mkdir()
    qualification_path.write_bytes(qualification)
    evidence_paths = {"release-qualification": "evidence/release-qualification.json"}
    if "worldstream-negotiate-evidence" in payloads:
        evidence_paths["worldstream-negotiate-evidence"] = payload_paths[
            "worldstream-negotiate-evidence"
        ]
    evidence_digests = {
        subject_id: "sha256:" + hashlib.sha256(payloads[subject_id]).hexdigest()
        for subject_id in evidence_paths
        if subject_id in payloads
    }
    evidence_digests["release-qualification"] = (
        "sha256:" + hashlib.sha256(qualification).hexdigest()
    )
    release = {
        "artifact_digests": artifact_digests,
        "artifacts": artifacts,
        "evidence": evidence_paths,
        "evidence_digests": evidence_digests,
        "manifest": {
            "mirror": "compatibility.json",
            "sha256": hashlib.sha256(compatibility_path.read_bytes()).hexdigest(),
            "source": "compatibility.toml",
        },
        "product": "0.1.0",
        "schema": STARTER.RELEASE_MANIFEST_SCHEMA,
        "source_version": "0.1.0",
        "verification_material": {"sigstore-bundle": {"path": "sigstore.bundle.json"}},
    }
    (release_dir / "release-manifest.json").write_bytes(canonical(release))
    (release_dir / "sigstore.bundle.json").write_bytes(
        canonical({"mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.3"})
    )

    activity_packs: list[dict[str, Any]]
    if custom:
        custom_bundle, custom_pack = pack_bytes("vendor.procurement")
        custom_bundle_path = tmp_path / "vendor-procurement.wspack"
        custom_evidence_path = tmp_path / "vendor-procurement-evidence.json"
        custom_bundle_path.write_bytes(custom_bundle)
        custom_evidence_path.write_bytes(canonical({"result": "passed"}))
        subjects.extend(
            [
                {
                    "binding": {"kind": "candidate", "path": custom_bundle_path.name},
                    "id": "vendor-procurement-bundle",
                    "license_expression": "Apache-2.0",
                    "media_type": "application/vnd.worldstream.wspack",
                    "role": "activity-pack-bundle",
                },
                {
                    "binding": {"kind": "candidate", "path": custom_evidence_path.name},
                    "id": "vendor-procurement-evidence",
                    "license_expression": "Apache-2.0",
                    "media_type": "application/json",
                    "role": "activity-pack-evidence",
                },
            ]
        )
        activity_packs = [
            {
                **custom_pack,
                "bundle_subject_id": "vendor-procurement-bundle",
                "evidence_subject_id": "vendor-procurement-evidence",
                "official": False,
            }
        ]
    else:
        activity_packs = [
            {
                **official_pack,
                "bundle_subject_id": "negotiate-bundle",
                "evidence_subject_id": "negotiate-evidence",
                "official": True,
            }
        ]
    inventory = {
        "activity_packs": activity_packs,
        "distribution": {
            "id": "worldstream-starter",
            "mode": "custom" if custom else "official",
            "profile": "native-linux-x86_64",
            "version": "0.1.0",
        },
        "schema": STARTER.CANDIDATE_SCHEMA,
        "subjects": subjects,
        "trust": {
            "compatibility_manifest": compatibility_path.name,
            "release_manifest": "release/release-manifest.json",
            "sigstore_bundle": "release/sigstore.bundle.json",
        },
    }
    inventory_path = tmp_path / "starter-candidate.json"
    inventory_path.write_bytes(canonical(inventory))
    return inventory_path, inventory


def rewrite_archive(
    source: Path,
    destination: Path,
    mutation,
) -> None:
    root, entries, epoch = STARTER.archive_entries(source)
    mutation(entries)
    STARTER.write_archive(destination, root, entries, epoch)


def test_official_archive_is_deterministic_and_offline_verifiable(
    tmp_path: Path,
) -> None:
    inventory_path, inventory = fixture(tmp_path)
    first = tmp_path / "first.tar.gz"
    second = tmp_path / "second.tar.gz"

    first_receipt = STARTER.build_archive(
        inventory_path, first, source_date_epoch=1724990400, structural_only=True
    )
    inventory["subjects"].reverse()
    inventory_path.write_bytes(canonical(inventory))
    second_receipt = STARTER.build_archive(
        inventory_path, second, source_date_epoch=1724990400, structural_only=True
    )

    assert first.read_bytes() == second.read_bytes()
    assert first_receipt["archive_sha256"] == second_receipt["archive_sha256"]
    assert first_receipt["authentication"] == "structural-only"
    verified = STARTER.verify_archive(first, structural_only=True)
    assert verified["status"] == "passed"
    assert verified["subject_count"] == len(STARTER.OFFICIAL_ROLES)


def test_checked_in_negotiate_bundle_and_evidence_have_the_frozen_identity() -> None:
    bundle = (
        ROOT / "packs/negotiate/releases/0.1.0/"
        "worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack"
    ).read_bytes()
    identity = STARTER.pack_identity(bundle, "checked-in Negotiate bundle")

    assert identity == {
        "bundle_digest": "blake3:9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5",
        "explanatory_version": "0.1.0",
        "pack_id": "worldstream.negotiate",
        "revision_digest": "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589",
    }
    STARTER.validate_official_evidence(
        (ROOT / "packs/negotiate/evidence/conformance-v1.json").read_bytes(),
        identity,
    )


def test_cli_structural_mode_is_explicitly_non_release(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    inventory_path, _inventory = fixture(tmp_path)
    archive = tmp_path / "starter.tar.gz"

    assert (
        STARTER.main(
            [
                "build",
                "--inventory",
                str(inventory_path),
                "--output",
                str(archive),
                "--source-date-epoch",
                "0",
                "--structural-only",
            ]
        )
        == 11
    )
    build_receipt = json.loads(capsys.readouterr().out)
    assert build_receipt["authentication"] == "structural-only"
    assert STARTER.main(["verify", str(archive), "--structural-only"]) == 11
    verify_receipt = json.loads(capsys.readouterr().out)
    assert verify_receipt["archive_sha256"] == build_receipt["archive_sha256"]


def test_custom_candidate_keeps_runtime_signed_and_pack_locally_approvable(
    tmp_path: Path,
) -> None:
    inventory_path, _inventory = fixture(tmp_path, custom=True)
    archive = tmp_path / "custom.tar.gz"

    STARTER.build_archive(
        inventory_path, archive, source_date_epoch=0, structural_only=True
    )
    _root, entries, _epoch = STARTER.archive_entries(archive)
    manifest = json.loads(entries["starter-manifest.json"])
    by_role = {row["role"]: row for row in manifest["subjects"]}

    assert by_role["runtime-distribution"]["authentication"] == "release-manifest"
    assert (
        by_role["activity-pack-bundle"]["authentication"]
        == "exact-digest-pending-local-approval"
    )
    assert manifest["policy"]["approval_state_carried"] is False
    assert manifest["policy"]["target_local_exact_digest_approval_required"] is True
    STARTER.verify_archive(archive, structural_only=True)


def test_verifier_rejects_substitution_and_extra_member(tmp_path: Path) -> None:
    inventory_path, _inventory = fixture(tmp_path)
    archive = tmp_path / "starter.tar.gz"
    STARTER.build_archive(
        inventory_path, archive, source_date_epoch=0, structural_only=True
    )
    substituted = tmp_path / "substituted.tar.gz"
    rewrite_archive(
        archive,
        substituted,
        lambda entries: entries.__setitem__(
            next(
                path
                for path in entries
                if path.startswith("payload/runtime-distribution/")
            ),
            b"substituted runtime\n",
        ),
    )
    with pytest.raises(STARTER.StarterError, match="substituted"):
        STARTER.verify_archive(substituted, structural_only=True)

    extra = tmp_path / "extra.tar.gz"
    rewrite_archive(
        archive,
        extra,
        lambda entries: entries.__setitem__("payload/unlisted.txt", b"extra\n"),
    )
    with pytest.raises(STARTER.StarterError, match="not closed"):
        STARTER.verify_archive(extra, structural_only=True)


def test_verifier_rejects_signed_subject_relabeling(tmp_path: Path) -> None:
    inventory_path, _inventory = fixture(tmp_path)
    archive = tmp_path / "starter.tar.gz"
    STARTER.build_archive(
        inventory_path, archive, source_date_epoch=0, structural_only=True
    )
    relabeled = tmp_path / "relabeled.tar.gz"

    def relabel(entries: dict[str, bytes]) -> None:
        manifest = json.loads(entries["starter-manifest.json"])
        runtime = next(
            row for row in manifest["subjects"] if row["role"] == "runtime-distribution"
        )
        console = next(
            row for row in manifest["subjects"] if row["role"] == "participant-console"
        )
        runtime["release_binding"] = console["release_binding"]
        runtime["sha256"] = console["sha256"]
        runtime["size_bytes"] = console["size_bytes"]
        entries[runtime["path"]] = entries[console["path"]]
        entries["starter-manifest.json"] = canonical(manifest)

    rewrite_archive(archive, relabeled, relabel)
    with pytest.raises(STARTER.StarterError, match="canonical role"):
        STARTER.verify_archive(relabeled, structural_only=True)


def test_verifier_rejects_path_traversal_member(tmp_path: Path) -> None:
    path = tmp_path / "traversal.tar.gz"
    with (
        path.open("wb") as output,
        gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed,
        tarfile.open(
            fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT
        ) as archive,
    ):
        info = tarfile.TarInfo("worldstream-starter-0.1.0-source/../escape")
        info.size = 1
        info.mode = 0o644
        info.uid = info.gid = 0
        info.mtime = 0
        archive.addfile(info, io.BytesIO(b"x"))

    with pytest.raises(STARTER.StarterError, match="safe relative path"):
        STARTER.verify_archive(path, structural_only=True)


def test_builder_rejects_release_subject_substitution(tmp_path: Path) -> None:
    inventory_path, _inventory = fixture(tmp_path)
    runtime = tmp_path / "release/subjects/runtime-distribution.tar.gz"
    runtime.write_bytes(b"substituted after signing\n")

    with pytest.raises(STARTER.StarterError, match="substituted"):
        STARTER.build_archive(
            inventory_path,
            tmp_path / "starter.tar.gz",
            source_date_epoch=0,
            structural_only=True,
        )


def test_builder_rejects_sensitive_candidate_path_and_identity_drift(
    tmp_path: Path,
) -> None:
    inventory_path, inventory = fixture(tmp_path, custom=True)
    sensitive = tmp_path / "approval.sqlite"
    sensitive.write_bytes((tmp_path / "vendor-procurement.wspack").read_bytes())
    bundle_subject = next(
        row for row in inventory["subjects"] if row["role"] == "activity-pack-bundle"
    )
    bundle_subject["binding"]["path"] = sensitive.name
    inventory_path.write_bytes(canonical(inventory))
    with pytest.raises(STARTER.StarterError, match="approval, credential"):
        STARTER.load_candidate(inventory_path)

    inventory_path, inventory = fixture(tmp_path / "drift", custom=True)
    inventory["activity_packs"][0]["bundle_digest"] = "blake3:" + "0" * 64
    inventory_path.write_bytes(canonical(inventory))
    with pytest.raises(STARTER.StarterError, match="differs from exact bundle bytes"):
        STARTER.load_candidate(inventory_path)
