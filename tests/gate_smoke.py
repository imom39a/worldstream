from __future__ import annotations

import importlib.util
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATES_PATH = ROOT / "scripts/gates.py"
WORKFLOW_PATH = ROOT / ".github/workflows/compatibility-gates.yml"


def load_gates():
    spec = importlib.util.spec_from_file_location("worldstream_gates_smoke", GATES_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load scripts/gates.py")
    gates = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = gates
    spec.loader.exec_module(gates)
    return gates


def detached_artifact_row(gates, artifact_id: str, profile: str) -> dict:
    if artifact_id == gates.SIGSTORE_VERIFICATION_ARTIFACT_ID:
        return {
            "id": artifact_id,
            "profile": profile,
            "digest_algorithm": "",
            "digest": "",
            "status": "verification_material",
            "verification_material_location": gates.SIGSTORE_VERIFICATION_LOCATION,
        }
    return {
        "id": artifact_id,
        "profile": profile,
        "digest_algorithm": "sha256",
        "digest": "",
        "status": "detached",
        "digest_location": "release-manifest.json",
    }


def test_cargo_evidence_rejects_zero_test_success(monkeypatch):
    gates = load_gates()
    commands: list[list[str]] = []

    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/cargo")

    def fake_run(argv, **kwargs):
        commands.append(argv)
        return subprocess.CompletedProcess(
            argv,
            0,
            stdout="test result: ok. 0 passed; 0 failed; 1 filtered out;\n",
            stderr="",
        )

    monkeypatch.setattr(gates, "_run_process_tree", fake_run)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.cargo_test_evidence(
        runner,
        "evidence-test",
        "worldstream-fixture",
        ("missing-filter",),
        "fixture boundary",
    )

    assert commands[0][0:4] == ["/usr/bin/cargo", "test", "--locked", "--offline"]
    assert commands[0][-2:] == ["--", "missing-filter"]
    assert runner.outcomes[0].status == "FAIL"
    assert "no passing test count" in runner.outcomes[0].detail


def test_windows_filesystem_policy_is_not_a_local_pass():
    gates = load_gates()
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.filesystem_checks(runner, host_os="nt")

    statuses = {outcome.name: outcome.status for outcome in runner.outcomes}
    assert statuses["filesystem-owner-only"] == "SKIP_INCOMPLETE"
    assert statuses["filesystem-acl-policy"] == "SKIP_INCOMPLETE"
    assert statuses["evidence-filesystem-local"] == "SKIP_INCOMPLETE"


def test_darwin_kill_point_is_incomplete_not_failure_or_evidence(monkeypatch):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    monkeypatch.setattr(gates.platform_module, "system", lambda: "Darwin")
    commands = []

    def fake_command(name, argv, **kwargs):
        commands.append((name, argv))
        return True

    monkeypatch.setattr(runner, "command", fake_command)
    gates.process_contract_checks(runner)

    kill = next(
        outcome for outcome in runner.outcomes if outcome.name == "process-kill-point"
    )
    assert kill.status == "SKIP_INCOMPLETE"
    assert kill.classification == "incomplete"
    assert not any(name == "process-kill-point" for name, _argv in commands)
    assert all(outcome.classification == "incomplete" for outcome in runner.outcomes)


def test_strict_local_hook_routes_missing_postgres_container_evidence_to_hosted_linux(
    monkeypatch,
):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    monkeypatch.setattr(gates.os, "name", "posix")
    monkeypatch.setattr(gates.shutil, "which", lambda _name: None)

    gates.postgres_live_contract(runner, required=False)

    outcome = next(
        item for item in runner.outcomes if item.name == "postgres-live-contract"
    )
    assert outcome.status == "SKIP_INCOMPLETE"
    assert outcome.classification == "incomplete"
    assert "hosted Linux owns" in outcome.detail
    assert runner.finish() == 0


def test_release_inventory_catches_unmapped_release_gate_evidence():
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    manifest = {
        "evidence": [
            {
                "id": "new-release-contract",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ]
    }

    gates.contract_inventory(runner, manifest, release=True)

    inventory = [
        outcome
        for outcome in runner.outcomes
        if outcome.name == "release-evidence-inventory"
    ]
    assert inventory and inventory[0].status == "FAIL"
    assert "new-release-contract" in inventory[0].detail


def test_ci_matrix_uses_explicit_platform_route(capsys):
    gates = load_gates()
    manifest = {
        "gate_cells": [
            {
                "id": "native-linux-x86_64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-linux-x86_64",
            }
        ]
    }

    assert gates.list_cells(manifest, "minimal-ci", "matrix") == 0
    assert '"runner":"ubuntu-24.04"' in capsys.readouterr().out


def test_workflow_bootstraps_exact_gate_python_before_running_offline_gate():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "cargo fetch --locked" in workflow
    assert "uv python install 3.14.7" in workflow
    assert (
        "uv run --python 3.14.7 --no-project python scripts/gates.py fast --ci --offline"
        in workflow
    )
    assert (
        "uv run --python 3.14.7 --no-project python scripts/gates.py minimal-ci"
        in workflow
    )


def test_build_type_contract_gate_validates_example_and_fails_closed(monkeypatch):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.build_type_contract_gate(runner)

    assert runner.outcomes[-1].name == "release-build-type-contract"
    assert runner.outcomes[-1].status == "PASS"

    identity = gates.release_build_identity_verifier()
    original_regular_bytes = identity.regular_bytes

    def tampered_regular_bytes(path, label, maximum=None):
        content = original_regular_bytes(path, label, maximum)
        if path == gates.ROOT / identity.BUILD_TYPE_EXAMPLE_PATH:
            value = identity.strict_json(content, label)
            value["predicate"]["unknown"] = True
            return identity.canonical_json(value)
        return content

    monkeypatch.setattr(identity, "regular_bytes", tampered_regular_bytes)
    monkeypatch.setattr(gates, "release_build_identity_verifier", lambda: identity)
    rejected = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.build_type_contract_gate(rejected)

    assert rejected.outcomes[-1].name == "release-build-type-contract"
    assert rejected.outcomes[-1].status == "FAIL"
    assert "illustrative graph" in rejected.outcomes[-1].detail


def test_release_digest_streams_large_sparse_files_enforces_bounds_and_reuses_cache(
    tmp_path, monkeypatch
):
    gates = load_gates()
    payload = tmp_path / "large-sparse-payload.oci.tar"
    with payload.open("wb") as stream:
        stream.seek(gates.MAX_RELEASE_CONTROL_BYTES)
        stream.write(b"\0")

    def reject_read_bytes(_path):
        raise AssertionError("release hashing must never materialize whole files")

    monkeypatch.setattr(Path, "read_bytes", reject_read_bytes)
    cache = {}
    digest = gates.bounded_release_sha256(
        payload, maximum=gates.MAX_RELEASE_PAYLOAD_BYTES, cache=cache
    )
    assert re.fullmatch(r"[0-9a-f]{64}", digest)
    assert len(cache) == 1

    def reject_second_open(*_args, **_kwargs):
        raise AssertionError("an unchanged release file must reuse its cached digest")

    monkeypatch.setattr(Path, "open", reject_second_open)
    assert (
        gates.bounded_release_sha256(
            payload, maximum=gates.MAX_RELEASE_PAYLOAD_BYTES, cache=cache
        )
        == digest
    )
    try:
        gates.bounded_release_sha256(
            payload, maximum=gates.MAX_RELEASE_CONTROL_BYTES, cache=cache
        )
    except gates.ReleaseHashError as error:
        assert "not regular and bounded" in str(error)
    else:  # pragma: no cover - fail-closed regression guard
        raise AssertionError("oversized release control file was accepted")


def test_release_digest_rejects_path_replacement_during_hashing(tmp_path, monkeypatch):
    gates = load_gates()
    payload = tmp_path / "payload.tar"
    replacement = tmp_path / "replacement.tar"
    payload.write_bytes(b"original")
    replacement.write_bytes(b"tampered")
    original_open = Path.open
    replaced = False

    class ReplacingStream:
        def __init__(self, stream):
            self.stream = stream

        def __enter__(self):
            self.stream.__enter__()
            return self

        def __exit__(self, *args):
            return self.stream.__exit__(*args)

        def fileno(self):
            return self.stream.fileno()

        def read(self, *args, **kwargs):
            nonlocal replaced
            if not replaced:
                replaced = True
                replacement.replace(payload)
            return self.stream.read(*args, **kwargs)

    def replace_path_after_open(path, *args, **kwargs):
        stream = original_open(path, *args, **kwargs)
        return ReplacingStream(stream) if path == payload else stream

    monkeypatch.setattr(Path, "open", replace_path_after_open)
    with pytest.raises(gates.ReleaseHashError, match="changed while hashing"):
        gates.bounded_release_sha256(
            payload, maximum=gates.MAX_RELEASE_CONTROL_BYTES, cache={}
        )


def test_workflow_keeps_native_postgres_provisioning_pinned_and_platform_specific():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "postgres:17.11-alpine@sha256:" in workflow
    assert "runner.os == 'Linux'" in workflow
    assert "runner.os == 'Windows'" in workflow
    assert "scripts/gates-install-postgres.ps1" in workflow


def test_ci_matrix_materializes_declared_oci_platform(capsys):
    gates = load_gates()
    manifest = {
        "gate_cells": [
            {
                "id": "native-linux-x86_64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-linux-x86_64",
            },
            {
                "id": "native-windows-x64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-windows-x64",
            },
        ],
        "platforms": [
            {
                "id": "oci-linux-amd64",
                "support": "release",
                "target": "linux/amd64",
                "storage_profiles": ["sqlite-bundled", "postgres-primary"],
            }
        ],
    }

    assert gates.list_cells(manifest, "minimal-ci", "matrix") == 0
    matrix = json.loads(capsys.readouterr().out)
    assert [row["cell"] for row in matrix["include"]] == [
        "native-linux-x86_64",
        "native-windows-x64",
        "oci-linux-amd64",
    ]
    oci = matrix["include"][-1]
    assert oci["platform"] == "oci-linux-amd64"
    assert oci["runner"] == "ubuntu-24.04"
    assert oci["shell"] == "bash"
    assert oci["system"] == "Linux"


def test_ci_platform_mismatch_is_an_exact_incomplete_blocker(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.platform_module, "system", lambda: "Darwin")
    runner = gates.GateRunner(strict=False, offline=True, ci=False)
    manifest = {
        "gate_cells": [
            {
                "id": "native-windows-x64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-windows-x64",
            }
        ]
    }

    assert gates.selected_ci_cell(runner, manifest, "native-windows-x64") is None
    outcome = runner.outcomes[0]
    assert outcome.status == "SKIP_INCOMPLETE"
    assert outcome.classification == "incomplete"
    assert "platform=native-windows-x64" in outcome.detail
    assert "requires runner=windows-2025-vs2026" in outcome.detail
    assert "observed system=Darwin" in outcome.detail


def test_release_handoff_preserves_unresolved_manifest_evidence():
    gates = load_gates()
    manifest = {
        "manifest_kind": "specification",
        "release_ready": False,
        "release_candidate": "0.1.0",
        "release_artifacts": [
            {
                "id": "oci-linux-amd64-image",
                "profile": "oci-linux-amd64",
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "unresolved",
            },
            {
                "id": "sigstore-bundle",
                "profile": "all",
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "unresolved",
            },
        ],
        "evidence": [
            {
                "id": "checksums-signature-sbom-provenance",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ],
    }

    handoff = gates.release_evidence_handoff(manifest)
    assert handoff["schema"] == "worldstream/release-evidence-handoff/v1"
    assert handoff["release_evidence"] is False
    assert handoff["release_ready"] is False
    artifacts = {row["id"]: row for row in handoff["required_artifacts"]}
    assert artifacts["oci-linux-amd64-image"]["path"] == (
        "worldstream-0.1.0-oci-linux-amd64.oci.tar"
    )
    assert artifacts["oci-linux-amd64-image"]["digest"] == ""
    assert artifacts["sigstore-bundle"]["path"] == "sigstore.bundle.json"
    assert handoff["required_evidence"][0]["artifact_digest"] == ""


def test_nonrelease_tiers_accept_coherent_fail_closed_specification():
    gates = load_gates()
    manifest = gates.load_manifest()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.manifest_gate(runner, manifest, release=False)

    state = [
        outcome
        for outcome in runner.outcomes
        if outcome.name == "manifest-contract-state"
    ]
    assert state and state[0].status == "PASS"
    assert (
        "detached release and qualification evidence is not asserted" in state[0].detail
    )


def test_release_metadata_requires_exact_inventory_and_digests(tmp_path, monkeypatch):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    (release_dir / "release-manifest.json").write_text(
        json.dumps(
            {
                "schema": "worldstream/release-artifact-manifest/v1",
                "product": "0.1.0",
                "source_version": "0.1.0",
                "manifest": {},
                "artifacts": {},
                "artifact_digests": {},
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {
            "release_candidate": "0.1.0",
            "release_artifacts": [
                {
                    "id": "source-archive",
                    "profile": "source",
                    "digest_algorithm": "sha256",
                    "digest": "",
                    "status": "unresolved",
                }
            ],
        },
    )

    assert any(
        outcome.name == "release-artifact-manifest" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )


def test_release_artifact_identity_diagnostic_keeps_placeholder_unresolved(
    tmp_path, monkeypatch
):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    artifact = release_dir / "worldstream-0.1.0-source.tar.gz"
    artifact.write_bytes(b"source fixture")
    observed = "sha256:" + gates.hashlib.sha256(artifact.read_bytes()).hexdigest()
    metadata = {
        "schema": "worldstream/release-artifact-manifest/v1",
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {"source-archive": artifact.name},
        "artifact_digests": {"source-archive": observed},
    }
    (release_dir / "release-manifest.json").write_text(
        json.dumps(metadata), encoding="utf-8"
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {
            "release_candidate": "0.1.0",
            "release_artifacts": [
                {
                    "id": "source-archive",
                    "profile": "source",
                    "digest_algorithm": "sha256",
                    "digest": "",
                    "status": "unresolved",
                }
            ],
        },
    )

    identity = next(
        outcome
        for outcome in runner.outcomes
        if outcome.name == "release-artifact-source-archive"
    )
    assert identity.status == "SKIP_INCOMPLETE"
    assert identity.classification == "incomplete"
    assert "profile='source'" in identity.detail
    assert "manifest_digest=''" in identity.detail
    assert f"observed_bytes='{observed}'" in identity.detail


def test_release_identity_failure_reports_expected_and_observed_values(
    tmp_path, monkeypatch
):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    (release_dir / "release-manifest.json").write_text(
        json.dumps(
            {
                "schema": "worldstream/release-artifact-manifest/v1",
                "product": "0.1.0",
                "source_version": "0.1.0",
                "manifest": {
                    "source": "wrong.toml",
                    "mirror": "compatibility.json",
                    "sha256": "0" * 64,
                },
                "artifacts": {},
                "artifact_digests": {},
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {"release_candidate": "0.1.0", "release_artifacts": []},
    )

    details = [
        outcome.detail
        for outcome in runner.outcomes
        if outcome.name == "release-manifest-identity"
    ]
    assert any("expected='compatibility.toml'" in detail for detail in details)
    assert any("observed='wrong.toml'" in detail for detail in details)
    assert any("expected='" in detail and "observed='" in detail for detail in details)


def test_command_start_failure_is_classified_as_gate_failure(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/tool")

    def fail_to_start(*args, **kwargs):
        raise OSError("simulated spawn failure")

    monkeypatch.setattr(gates, "_run_process_tree", fail_to_start)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    assert not runner.command("spawn-test", ["tool"])
    assert runner.outcomes[0].status == "FAIL"
    assert "could not start" in runner.outcomes[0].detail


def test_gate_runner_enforces_manifest_deadline_on_commands_and_finish(monkeypatch):
    gates = load_gates()
    clock = [100.0]
    monkeypatch.setattr(gates.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(gates.shutil, "which", lambda _name: "/usr/bin/tool")
    observed = {}

    def complete(argv, **kwargs):
        observed["argv"] = argv
        observed["timeout"] = kwargs["timeout"]
        return gates.subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(gates, "_run_process_tree", complete)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    runner.configure_deadline({"evidence_tiers": {"fast": {"hard_seconds": 2}}}, "fast")

    assert runner.command("bounded", ["tool"])
    assert observed == {"argv": ["tool"], "timeout": 2.0}
    clock[0] = 102.1
    assert runner.finish() == 1
    deadline = [row for row in runner.outcomes if row.name == "tier-hard-deadline"]
    assert len(deadline) == 1
    assert deadline[0].status == "FAIL"
    assert "exceeded 2 seconds" in deadline[0].detail


def test_release_gate_uses_the_earlier_workflow_producer_deadline(monkeypatch):
    gates = load_gates()
    monotonic = [500.0]
    wall = [1_700_000_000.0]
    monkeypatch.setattr(gates.time, "monotonic", lambda: monotonic[0])
    monkeypatch.setattr(gates.time, "time", lambda: wall[0])
    monkeypatch.setenv(gates.RELEASE_DEADLINE_EPOCH_ENV, "1700000030")
    runner = gates.GateRunner(strict=True, offline=True, ci=True)

    runner.configure_deadline(
        {"evidence_tiers": {"release": {"hard_seconds": 14_400}}}, "release"
    )

    assert runner.deadline_monotonic == 530.0
    assert runner.deadline_source == "workflow_release_producer_start"
    monotonic[0] = 530.1
    assert runner.finish() == 1
    assert runner.outcomes[-1].status == "FAIL"
    assert "workflow_release_producer_start" in runner.outcomes[-1].detail


@pytest.mark.parametrize("value", ["not-an-epoch", "0"])
def test_release_gate_rejects_invalid_or_expired_workflow_deadline(monkeypatch, value):
    gates = load_gates()
    monkeypatch.setenv(gates.RELEASE_DEADLINE_EPOCH_ENV, value)
    monkeypatch.setattr(gates.time, "time", lambda: 1_700_000_000.0)
    runner = gates.GateRunner(strict=True, offline=True, ci=True)

    runner.configure_deadline(
        {"evidence_tiers": {"release": {"hard_seconds": 14_400}}}, "release"
    )

    assert runner.deadline_recorded
    assert runner.outcomes[-1].status == "FAIL"


def test_gate_runner_terminates_a_command_at_the_tier_deadline(monkeypatch):
    gates = load_gates()
    clock = [200.0]
    monkeypatch.setattr(gates.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(gates.shutil, "which", lambda _name: "/usr/bin/tool")

    def time_out(argv, **kwargs):
        raise gates.subprocess.TimeoutExpired(argv, kwargs["timeout"])

    monkeypatch.setattr(gates, "_run_process_tree", time_out)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    runner.configure_deadline(
        {"evidence_tiers": {"minimal_ci": {"hard_seconds": 1}}}, "minimal-ci"
    )

    assert not runner.command("bounded", ["tool"])
    assert runner.outcomes[-1].status == "FAIL"
    assert "exhausted the tier hard deadline" in runner.outcomes[-1].detail


def _pid_is_running(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    if sys.platform.startswith("linux"):
        try:
            state = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8").split()[2]
        except (OSError, IndexError):
            return False
        return state != "Z"
    return True


@pytest.mark.skipif(os.name != "posix", reason="POSIX PID identity regression")
def test_posix_signal_revalidates_identity_immediately_before_every_signal(
    monkeypatch,
):
    gates = load_gates()
    identities = iter(["owned-start", "owned-start", "reused-start"])
    signaled: list[tuple[int, int]] = []
    monkeypatch.setattr(gates, "_posix_process_identity", lambda _pid: next(identities))
    monkeypatch.setattr(
        gates.os,
        "kill",
        lambda pid, signal_number: signaled.append((pid, signal_number)),
    )

    assert gates._signal_posix_process(5678, "owned-start", signal.SIGSTOP)
    assert gates._signal_posix_process(5678, "owned-start", signal.SIGTERM)
    assert not gates._signal_posix_process(5678, "owned-start", signal.SIGKILL)
    assert signaled == [(5678, signal.SIGSTOP), (5678, signal.SIGTERM)]


@pytest.mark.skipif(os.name != "posix", reason="POSIX process-group regression")
def test_posix_cleanup_never_signals_a_reaped_roots_old_process_group(monkeypatch):
    gates = load_gates()

    class CompletedProcess:
        pid = 1234

        @staticmethod
        def poll():
            return 0

    freeze_results = iter([{5678: "owned-start"}, {5678: "owned-start"}])
    signals: list[tuple[int, int]] = []
    monkeypatch.setattr(
        gates,
        "_freeze_posix_processes",
        lambda _marker, _seeded=None: next(freeze_results),
    )
    monkeypatch.setattr(
        gates,
        "_signal_posix_process",
        lambda pid, _started, signal_number: (
            signals.append((pid, signal_number)) or True
        ),
    )
    monkeypatch.setattr(
        gates,
        "_wait_posix_processes_gone",
        lambda identities, **_kwargs: (
            {}
            if signal.SIGKILL in {signal_number for _pid, signal_number in signals}
            else dict(identities)
        ),
    )
    monkeypatch.setattr(
        gates.os,
        "killpg",
        lambda *_args: (_ for _ in ()).throw(
            AssertionError("a completed root's PGID must never be signaled")
        ),
        raising=False,
    )

    gates._terminate_posix_processes(CompletedProcess(), b"marker=value", None)

    assert signals == [
        (5678, signal.SIGTERM),
        (5678, signal.SIGCONT),
        (5678, signal.SIGKILL),
    ]


@pytest.mark.skipif(os.name != "posix", reason="POSIX PID identity regression")
def test_posix_wait_does_not_drop_a_reused_root_pid(monkeypatch):
    gates = load_gates()

    class ReapedRoot:
        pid = 1234

        @staticmethod
        def poll():
            return 0

    monkeypatch.setattr(
        gates,
        "_posix_process_identity",
        lambda _pid: "reused-start",
    )

    remaining = gates._wait_posix_processes_gone(
        {1234: "reused-start"},
        timeout=0,
        root_process=ReapedRoot(),
        root_started="original-start",
    )

    assert remaining == {1234: "reused-start"}


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin native sampler")
def test_darwin_process_snapshot_does_not_launch_ps(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(
        gates.subprocess,
        "Popen",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("Darwin process sampling must not launch ps")
        ),
    )

    processes = gates._posix_process_snapshot()

    assert os.getpid() in processes


@pytest.mark.skipif(os.name != "posix", reason="real POSIX process-tree regression")
def test_gate_runner_kills_and_reaps_adversarial_child_and_detached_grandchild(
    tmp_path,
):
    gates = load_gates()
    identities = tmp_path / "process-tree.txt"
    child = (
        "import os,pathlib,subprocess,sys,time; "
        "grandchild=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],"
        "start_new_session=True); "
        "pathlib.Path(sys.argv[1]).write_text(f'{os.getpid()} {grandchild.pid}',encoding='utf-8'); "
        "time.sleep(60)"
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    runner.configure_deadline({"evidence_tiers": {"fast": {"hard_seconds": 1}}}, "fast")

    assert (
        runner.bounded_process(
            "real-process-tree-timeout",
            [sys.executable, "-c", child, str(identities)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        is None
    )
    child_pid, grandchild_pid = (
        int(value) for value in identities.read_text(encoding="utf-8").split()
    )
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and any(
        _pid_is_running(pid) for pid in (child_pid, grandchild_pid)
    ):
        time.sleep(0.02)
    assert not _pid_is_running(child_pid)
    assert not _pid_is_running(grandchild_pid)
    assert runner.outcomes[-1].status == "FAIL"
    assert "exhausted the tier hard deadline" in runner.outcomes[-1].detail


@pytest.mark.skipif(os.name != "posix", reason="real POSIX process-tree regression")
def test_gate_runner_reaps_detached_child_after_successful_parent_exit(tmp_path):
    gates = load_gates()
    identity = tmp_path / "detached-child.txt"
    parent = (
        "import pathlib,subprocess,sys,time; "
        "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],"
        "start_new_session=True); "
        "pathlib.Path(sys.argv[1]).write_text(str(child.pid),encoding='utf-8'); "
        "time.sleep(0.25)"
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    completed = runner.bounded_process(
        "real-process-tree-success",
        [sys.executable, "-c", parent, str(identity)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    assert completed is not None
    assert completed.returncode == 0
    child_pid = int(identity.read_text(encoding="utf-8"))
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and _pid_is_running(child_pid):
        time.sleep(0.02)
    assert not _pid_is_running(child_pid)


@pytest.mark.skipif(os.name != "posix", reason="real POSIX process-tree regression")
def test_gate_runner_reaps_rapid_daemon_after_successful_parent_exit(tmp_path):
    gates = load_gates()
    identity = tmp_path / "rapid-daemon.txt"
    parent = (
        "import pathlib,subprocess,sys; "
        "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],"
        "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,"
        "stderr=subprocess.DEVNULL,start_new_session=True); "
        "pathlib.Path(sys.argv[1]).write_text(str(child.pid),encoding='utf-8')"
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    completed = runner.bounded_process(
        "real-process-tree-rapid-daemon",
        [sys.executable, "-c", parent, str(identity)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    assert completed is not None
    assert completed.returncode == 0
    child_pid = int(identity.read_text(encoding="utf-8"))
    child_started = gates._posix_process_identity(child_pid)
    try:
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline and _pid_is_running(child_pid):
            time.sleep(0.01)
        assert not _pid_is_running(child_pid)
    finally:
        if child_started is not None:
            gates._signal_posix_process(child_pid, child_started, signal.SIGKILL)


@pytest.mark.skipif(os.name != "nt", reason="real Windows Job Object regression")
def test_gate_runner_windows_job_kills_and_reaps_child_and_grandchild(tmp_path):
    gates = load_gates()
    identities = tmp_path / "process-tree.txt"
    child = (
        "import os,pathlib,subprocess,sys,time; "
        "flags=getattr(subprocess,'CREATE_NEW_PROCESS_GROUP',0); "
        "grandchild=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],"
        "creationflags=flags); "
        "pathlib.Path(sys.argv[1]).write_text(f'{os.getpid()} {grandchild.pid}',encoding='utf-8'); "
        "time.sleep(60)"
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    runner.configure_deadline({"evidence_tiers": {"fast": {"hard_seconds": 1}}}, "fast")

    assert (
        runner.bounded_process(
            "real-windows-process-tree-timeout",
            [sys.executable, "-c", child, str(identities)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        is None
    )
    child_pid, grandchild_pid = (
        int(value) for value in identities.read_text(encoding="utf-8").split()
    )
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and any(
        _pid_is_running(pid) for pid in (child_pid, grandchild_pid)
    ):
        time.sleep(0.02)
    assert not _pid_is_running(child_pid)
    assert not _pid_is_running(grandchild_pid)
    assert runner.outcomes[-1].status == "FAIL"


def test_process_tree_containment_failure_is_a_closed_gate_failure(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda _name: "/usr/bin/tool")

    def fail_containment(*_args, **_kwargs):
        raise gates.ProcessTreeError("simulated containment failure")

    monkeypatch.setattr(gates, "_run_process_tree", fail_containment)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    assert not runner.command("tree-containment", ["tool"])
    assert runner.outcomes[-1].status == "FAIL"
    assert "process-tree containment failed" in runner.outcomes[-1].detail


def test_windows_job_setup_failure_kills_child_without_posix_cleanup(monkeypatch):
    gates = load_gates()

    class SuspendedProcess:
        pid = 42
        _handle = 7

        def __init__(self):
            self.killed = False
            self.waited = False

        def kill(self):
            self.killed = True

        def wait(self, *, timeout):
            assert timeout == gates.PROCESS_TREE_REAP_SECONDS
            self.waited = True
            return 1

        def poll(self):
            return 1 if self.waited else None

    process = SuspendedProcess()
    monkeypatch.setattr(gates.os, "name", "nt")
    monkeypatch.setattr(gates.subprocess, "Popen", lambda *_args, **_kwargs: process)

    def reject_job(_process):
        raise gates.ProcessTreeError("Job Object unavailable")

    monkeypatch.setattr(gates, "_WindowsProcessJob", reject_job)
    monkeypatch.setattr(
        gates.os,
        "killpg",
        lambda *_args: (_ for _ in ()).throw(
            AssertionError("Windows cleanup must never call POSIX killpg")
        ),
    )

    with pytest.raises(gates.ProcessTreeError, match="Job Object unavailable"):
        gates._run_process_tree(["tool"], timeout=1)
    assert process.killed is True
    assert process.waited is True


def test_pre_push_requires_a_positive_hard_deadline():
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    runner.configure_deadline({"evidence_tiers": {"pre_push": {}}}, "pre-push")

    assert runner.deadline_recorded
    assert runner.outcomes[-1].name == "tier-hard-deadline"
    assert runner.outcomes[-1].status == "FAIL"
    assert "pre-push has no positive integer" in runner.outcomes[-1].detail


def test_fast_tier_uses_static_python_checks_without_network_or_full_suite(
    monkeypatch,
):
    gates = load_gates()
    calls = []

    def record(name):
        return lambda *_args, **_kwargs: calls.append(name)

    for name in (
        "manifest_gate",
        "version_and_source_drift",
        "secret_scan",
        "root_python_static_checks",
        "rust_checks",
        "critical_contract_matrix",
    ):
        monkeypatch.setattr(gates, name, record(name))

    def forbidden(*_args, **_kwargs):
        raise AssertionError("fast tier crossed its declared bounded scope")

    monkeypatch.setattr(gates, "dependency_scan", forbidden)
    monkeypatch.setattr(gates, "root_python_checks", forbidden)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    assert (
        gates.run_tier(
            runner,
            {"evidence_tiers": {"fast": {"hard_seconds": 60}}},
            "fast",
            None,
        )
        == 0
    )
    assert calls == [
        "manifest_gate",
        "version_and_source_drift",
        "secret_scan",
        "root_python_static_checks",
        "rust_checks",
        "critical_contract_matrix",
    ]


def test_pre_commit_routes_through_fast_gate_launcher() -> None:
    hook = (ROOT / ".githooks/pre-commit").read_text(encoding="utf-8")
    staged_check = hook.index("git diff --cached --check")
    fast_gate = hook.index("scripts/gates.sh fast --offline")

    assert staged_check < fast_gate
    assert "python3 scripts/verify-manifest.py" not in hook
    assert "RUSTUP_TOOLCHAIN=" not in hook


@pytest.mark.parametrize("offline", [False, True])
def test_dependency_scan_disables_cargo_audit_fetch_when_offline(
    monkeypatch, offline
) -> None:
    gates = load_gates()
    monkeypatch.setattr(
        gates.shutil,
        "which",
        lambda name: "/tools/cargo-audit" if name == "cargo-audit" else None,
    )
    runner = gates.GateRunner(strict=True, offline=offline, ci=False)
    commands: list[tuple[str, list[str]]] = []
    monkeypatch.setattr(
        runner,
        "command",
        lambda name, argv, **_kwargs: commands.append((name, argv)) or True,
    )

    gates.dependency_scan(runner, release=False, include_ecosystems=False)

    audit = next(argv for name, argv in commands if name == "cargo-audit")
    assert ("--no-fetch" in audit) is offline


def test_gate_report_summarizes_blocking_and_incomplete_outcomes(tmp_path, monkeypatch):
    gates = load_gates()
    report_path = tmp_path / "gate-report.json"
    monkeypatch.setenv("WORLDSTREAM_GATE_REPORT", str(report_path))
    runner = gates.GateRunner(strict=False, offline=True, ci=False)
    runner.fail("release-artifacts", "unresolved artifact digest")
    runner.skip("native-windows", "Windows host is unavailable")

    assert runner.finish() == 1
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["summary"] == {
        "checks": 2,
        "failures": 1,
        "incomplete_skips": 1,
    }
    assert report["blocking_failures"][0]["name"] == "release-artifacts"
    assert report["incomplete_skips"][0]["name"] == "native-windows"
    assert report["incomplete_blockers"][0]["classification"] == "incomplete"
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True


def test_release_gate_report_contains_fail_closed_blocker_matrix(tmp_path):
    gates = load_gates()
    report_path = tmp_path / "release-gate-report.json"
    manifest = {
        "manifest_kind": "specification",
        "release_ready": False,
        "unresolved_required_fields": ["release_artifacts[*].digest"],
        "release_artifacts": [
            {
                "id": "native-windows-x64-archive",
                "status": "unresolved",
                "digest": "",
            }
        ],
        "evidence": [
            {
                "id": "native-windows-release-profile",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ],
    }
    runner = gates.GateRunner(
        strict=True,
        offline=True,
        ci=False,
        tier="release",
        report_path=str(report_path),
    )

    assert runner.finish(manifest=manifest, tier="release") == 1
    report = json.loads(report_path.read_text(encoding="utf-8"))
    matrix = report["release_blocker_matrix"]
    assert matrix["schema"] == "worldstream/release-blocker-matrix/v1"
    assert matrix["status"] == "blocked"
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True
    rows = {row["id"]: row for row in matrix["rows"]}
    assert rows["manifest-state"]["status"] == "blocked"
    assert rows["release-artifact:native-windows-x64-archive"]["status"] == "blocked"
    assert (
        rows["release-evidence:native-windows-release-profile"]["status"] == "blocked"
    )
    assert (
        "native Windows"
        in rows["release-artifact:native-windows-x64-archive"]["environment"]
    )
    assert not any(
        row["id"] == "release-artifact:native-windows-x64-archive"
        and row["status"] == "ready"
        for row in matrix["rows"]
    )


def test_release_report_is_fail_closed_for_malformed_manifest_collections(tmp_path):
    gates = load_gates()
    report_path = tmp_path / "malformed-release-report.json"
    runner = gates.GateRunner(
        strict=True,
        offline=True,
        ci=False,
        tier="release",
        report_path=str(report_path),
    )

    assert (
        runner.finish(
            manifest={
                "manifest_kind": "specification",
                "release_ready": False,
                "release_artifacts": None,
                "evidence": None,
                "platforms": None,
            },
            tier="release",
        )
        == 1
    )
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True
    assert report["release_blocker_matrix"]["status"] == "blocked"


def test_detached_release_rows_require_verified_artifact_and_evidence_outcomes():
    gates = load_gates()
    manifest = {
        "manifest_kind": "release",
        "release_ready": True,
        "unresolved_required_fields": [],
        "release_artifact_digest_source": "detached_release_manifest",
        "release_artifacts": [
            {
                "id": "native-windows-x64-archive",
                "profile": "native-windows-x64",
                "digest_algorithm": "sha256",
                "status": "detached",
                "digest": "",
                "digest_location": "release-manifest.json",
            }
        ],
        "evidence": [
            {
                "id": "native-windows-release-profile",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    declared_only = gates.release_blocker_matrix(manifest, [])
    declared_rows = {row["id"]: row for row in declared_only["rows"]}
    assert (
        declared_rows["release-artifact:native-windows-x64-archive"]["status"]
        == "blocked"
    )
    assert (
        declared_rows["release-evidence:native-windows-release-profile"]["status"]
        == "blocked"
    )

    verified = gates.release_blocker_matrix(
        manifest,
        [
            gates.Outcome(
                "release-artifact-native-windows-x64-archive",
                "PASS",
                "exact detached artifact digest verified",
                "pass",
            ),
            gates.Outcome(
                "release-evidence-native-windows-release-profile",
                "PASS",
                "exact detached evidence digest verified",
                "pass",
            ),
        ],
    )
    verified_rows = {row["id"]: row for row in verified["rows"]}
    assert (
        verified_rows["release-artifact:native-windows-x64-archive"]["status"]
        == "ready"
    )
    assert (
        verified_rows["release-evidence:native-windows-release-profile"]["status"]
        == "ready"
    )


def test_macos_quickstart_is_locked_and_reports_missing_tooling():
    script = (ROOT / "scripts/macos-source-quickstart.sh").read_text(encoding="utf-8")
    assert "missing_commands=()" in script
    assert "for command_name in cargo git node python3 pnpm rustc uv; do" in script
    assert "uv sync --project sdk/python --locked" in script
    assert "uv run --project sdk/python --locked pytest -q" in script
    assert "pnpm install --frozen-lockfile" in script
    assert "pnpm --dir web/console test" in script
    assert "WORLDSTREAM_BROWSER_MODE=cdp" in script
    assert "web/console/live-browser-story.sh" in script
    assert "quickstart_elapsed_seconds >= 600" in script
    assert 'getattr(os, "O_NOFOLLOW", 0)' in script
    assert "source.read(maximum_browser_story_bytes + 1)" in script
    assert "browser story input changed during bounded read" in script
    assert script.index("if not isinstance(browser_story, dict):") < script.index(
        'browser_story.get("schema")'
    )
    checked = subprocess.run(
        ["bash", "-n", str(ROOT / "scripts/macos-source-quickstart.sh")],
        check=False,
        capture_output=True,
        text=True,
    )
    assert checked.returncode == 0, checked.stderr


def test_fast_ci_primes_locked_rust_dependencies_before_offline_gate():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    fetch = workflow.index("cargo fetch --locked")
    offline_gate = workflow.index(
        "uv run --python 3.14.7 --no-project python scripts/gates.py fast --ci --offline"
    )

    assert fetch < offline_gate


def test_native_ci_provisions_exact_postgresql_before_manifest_cells():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    gate = workflow.index(
        "uv run --python 3.14.7 --no-project python scripts/gates.py minimal-ci --ci --cell"
    )

    assert workflow.index("postgres:17.11-alpine@sha256:") < gate
    assert workflow.index("scripts/gates-install-postgres.ps1") < gate
    assert "WORLDSTREAM_POSTGRES_URL=" not in workflow
    windows = (ROOT / "scripts/gates-install-postgres.ps1").read_text(encoding="utf-8")
    hosted_provisioning = workflow + windows
    assert hosted_provisioning.count("WORLDSTREAM_POSTGRES_DSN_FILE=") == 2
    assert hosted_provisioning.count("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=") == 2
    assert 'chmod 600 "$postgres_dsn_file"' in workflow
    assert "POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password" in workflow
    assert (
        "scripts/gates.py minimal-ci --ci --cell ${{ matrix.cell }} --report "
        "reports/${{ matrix.cell }}.json --offline" in workflow
    )


def test_native_linux_and_windows_execute_real_process_tree_regressions():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    native = workflow[workflow.index("  native:") : workflow.index("  macos-source:")]
    step = (
        "      - name: Prove native gate process-tree deadline containment\n"
        "        if: ${{ matrix.platform != 'oci-linux-amd64' }}\n"
        "        shell: ${{ matrix.shell }}\n"
        "        run: uv run --project sdk/python --locked python -m pytest -q "
        'tests/gate_smoke.py -k "test_gate_runner_kills_and_reaps_adversarial_'
        "child_and_detached_grandchild or test_gate_runner_windows_job_kills_and_"
        'reaps_child_and_grandchild"\n'
    )

    assert native.count(step) == 1


def test_pull_request_runner_matrix_is_static_workflow_data():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    native = workflow[workflow.index("  native:") : workflow.index("  macos-source:")]

    assert "fromJSON(needs.cells.outputs.matrix)" not in workflow
    assert "Resolve manifest gate cells" not in workflow
    assert native.count("runner: ubuntu-24.04") == 2
    assert native.count("runner: windows-2025-vs2026") == 1
    assert "self-hosted" not in native


def test_windows_ci_installs_and_proves_byte_pinned_postgresql_17_11():
    script = (ROOT / "scripts/gates-install-postgres.ps1").read_text(encoding="utf-8")
    assert "postgresql-17.11-1-windows-x64-binaries.zip" in script
    assert "6eabdf00d2893713b75db4336a23c3fdf505f056e217ec6e2e95d901750cfea3" in script
    assert "https://get.enterprisedb.com/postgresql/" in script
    assert "--pwfile $BootstrapSecret" in script
    assert "postgres (PostgreSQL) $PostgresVersion" in script
    assert "'170011'" in script
    assert "WORLDSTREAM_POSTGRES_DSN_FILE=" in script
    assert "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=" in script
    assert "WORLDSTREAM_POSTGRES_PASSWORD_FILE=" in script
    assert "WORLDSTREAM_PG_DUMP=" in script
    assert "WORLDSTREAM_PG_RESTORE=" in script
    assert "WORLDSTREAM_PSQL=" in script
    assert "POSTGRES_PASSWORD=" not in script
    assert "--superpassword" not in script.lower()
    assert "function Protect-WorldstreamPath" in script
    assert '"*S-1-5-18:$Permission"' in script
    assert '"*S-1-5-32-544:$Permission"' in script
    assert "if ($Directory) { '(OI)(CI)(F)' } else { '(F)' }" in script
    assert "Protect-WorldstreamPath -Path $SecretRoot -Directory" in script
    assert "Protect-WorldstreamPath -Path $BootstrapSecret" in script
    assert "Protect-WorldstreamPath -Path $PostgresDsnFile" in script
    secret_root_dacl = script.index(
        "Protect-WorldstreamPath -Path $SecretRoot -Directory"
    )
    secret_write = script.index("[IO.File]::WriteAllText($BootstrapSecret")
    secret_file_dacl = script.index("Protect-WorldstreamPath -Path $BootstrapSecret")
    assert secret_root_dacl < secret_write < secret_file_dacl
    assert (
        "could not apply the protected owner/SYSTEM/Administrators PostgreSQL credential DACL"
        in script
    )


def test_linux_release_installs_byte_pinned_postgresql_17_11_clients():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    script = (ROOT / "scripts/gates-install-postgres-client.sh").read_text(
        encoding="utf-8"
    )

    assert "scripts/gates-install-postgres-client.sh" in workflow
    assert "postgresql-client-17_17.11-1.pgdg24.04+2_amd64.deb" in script
    assert "postgresql-client-common_293.pgdg24.04+1_all.deb" in script
    assert "libpq5_18.6-1.pgdg24.04+2_amd64.deb" in script
    for digest in (
        "b3b071b67a814a382516d6f6241b529c52f909672457e2f48083acce047b634f",
        "80ae115f63ba67fbba442f6b75a468a559b77a3ee3b0be357a4c49e70bf860c5",
        "b487c5ed2ceb9244c6a9d6ae65818ed6707c3c44a2e14488394ae4194c52c53b",
    ):
        assert digest in script
    assert "--proto '=https' --tlsv1.2" in script
    assert "sha256sum --check --strict" in script
    assert "WORLDSTREAM_PG_DUMP=" in script
    assert "WORLDSTREAM_PG_RESTORE=" in script
    assert "WORLDSTREAM_PSQL=" in script


def test_release_workflow_uses_verified_safe_package_extraction():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    reference = workflow[
        workflow.index("  reference-performance-release:") : workflow.index(
            "  release-evidence:"
        )
    ]
    assert "scripts/release-package-extract.py" in reference
    assert "tar -xzf" not in reference
    assert "tar --extract" not in reference
    assert "scripts/verify-release.sh" not in reference
    assert '--sdk-output "$sdk_root"' in reference
    assert "reports/reference-package-extraction.json" in reference


def test_workflow_deadlines_cover_setup_and_the_complete_release_producer_chain():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")

    def job(name: str) -> str:
        start = workflow.index(f"  {name}:\n")
        following = re.search(r"\n  [a-z][a-z0-9-]+:\n", workflow[start + 1 :])
        end = len(workflow) if following is None else start + 1 + following.start()
        return workflow[start:end]

    assert "    timeout-minutes: 1\n" in job("fast")
    assert (
        "    timeout-minutes: ${{ github.event_name == 'workflow_dispatch' "
        "&& inputs.release == true && github.ref == 'refs/heads/main' && 60 || 25 }}\n"
        in job("native")
    )
    clock = job("release-clock")
    assert "deadline_epoch_seconds=$((started_epoch_seconds + 14400))" in clock
    for name in (
        "packaged-backend-release",
        "failure-soak-release",
        "reference-performance-release",
        "release-evidence",
        "release-signing",
        "release-finalize",
        "release-manifest-signing",
        "release-verify",
    ):
        assert "release-clock" in next(
            line for line in job(name).splitlines() if line.startswith("    needs:")
        )
    assert (
        "WORLDSTREAM_RELEASE_DEADLINE_EPOCH_SECONDS: "
        "${{ needs.release-clock.outputs.deadline_epoch_seconds }}"
        in job("release-verify")
    )


def test_release_job_bootstraps_pinned_gate_toolchain_before_release_gate():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    release = workflow[workflow.index("  release-evidence:") :]
    assert "astral-sh/setup-uv@" in release
    assert "uv python install 3.14.7" in release
    assert "rustup toolchain install 1.97.1" in release
    assert "actions/setup-node@" in release
    assert "corepack install" in release
    assert release.index("uv python install 3.14.7") < release.index(
        "scripts/gates.py release --release-inventory"
    )
    assert "id-token: write" in release
    assert "id-token: write" not in workflow[: workflow.index("  release-evidence:")]
    assert (
        "scripts/gates.py release --release-inventory "
        '"$WORLDSTREAM_RELEASE_INVENTORY" --ci --strict '
        "--report reports/release-gate.json "
        "--handoff reports/release-evidence-handoff.json --offline" in release
    )


def test_every_external_workflow_action_uses_an_exact_commit():
    gates = load_gates()
    identity = gates.release_build_identity_verifier()
    dependencies = identity.workflow_action_dependencies(
        identity.source_entries_from_root(ROOT)
    )

    assert dependencies
    assert all(
        item["uri"].startswith("https://github.com/")
        and re.fullmatch(r"[0-9a-f]{40}", item["digest"]["gitCommit"])
        for item in dependencies
    )


def test_release_workflow_builds_only_supported_distribution_surfaces():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    release = workflow[workflow.index("  release-evidence:") :]
    assert "--target linux-x86_64" in workflow
    assert "--target windows-x64" in workflow
    assert "--target source" in release
    assert (
        "docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e"
        in workflow
    )
    assert "version: v0.36.1" in workflow
    assert "driver: docker-container" in workflow
    assert "docker buildx build" in workflow
    assert "--platform linux/amd64" in workflow
    assert "--load" in workflow
    assert "--provenance=false" in workflow
    assert (
        '--output "type=oci,dest=$artifact,compression=gzip,force-compression=true"'
        in workflow
    )
    assert '--build-arg "SOURCE_DATE_EPOCH=0"' in workflow
    assert "scripts/oci-runtime-smoke.sh" in workflow
    assert '--artifact "$artifact"' in workflow
    assert '--image "$image_tag"' in workflow
    assert "--skip-build" in workflow
    assert "--report reports/oci-runtime-smoke.json" in workflow
    assert "scripts/verify-release.sh dist" in release
    assert "release-evidence-assemble.sh" in release
    assert "release-supply-chain.py" in release
    assert "SLSA provenance" in release
    assert "sigstore/cosign-installer@" in release
    assert "cosign-release: v3.1.3" in release
    assert "cosign sign-blob --yes" in release
    assert "bundle=signing-output/sigstore.bundle.json" in release
    assert 'cp --no-clobber "$bundle" dist/sigstore.bundle.json' in release
    assert "dist/release-manifest.json" in release


def test_workflow_types_the_exact_dual_profile_oci_runtime_report():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    native = workflow[workflow.index("  native:") : workflow.index("  macos-source:")]
    smoke = native.index("scripts/oci-runtime-smoke.sh")
    diagnostic = native.index("scripts/release-platform-diagnostic.py oci")
    producer = native.index("--report oci-linux/oci=")

    assert smoke < diagnostic < producer
    assert "--report reports/oci-runtime-smoke.json" in native[smoke:diagnostic]
    assert "--context dist/oci-context" in native[diagnostic:producer]
    assert "--runtime-report reports/oci-runtime-smoke.json" in native[diagnostic:]
    assert '--artifact "$artifact"' in native[smoke:diagnostic]
    assert "--skip-build" in native[smoke:diagnostic]


def test_local_postgresql_redacts_keyword_dsn_credentials(monkeypatch, capsys):
    gates = load_gates()
    dsn = "host=db.internal port=5432 dbname=worldstream user=operator password=SECRET"
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_URL", dsn)
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/psql")
    monkeypatch.setattr(
        gates,
        "_run_process_tree",
        lambda argv, **kwargs: subprocess.CompletedProcess(
            argv, 0, stdout="", stderr=""
        ),
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    output = capsys.readouterr().out
    assert "SECRET" not in output
    assert "password=SECRET" not in output
    assert "host=db.internal port=5432 dbname=worldstream" not in output
    assert "explicit local environment" in output
    redacted_uri = gates.redact_command_argument(
        "postgresql" + "://operator:" + "SECRET@db.internal:5432/worldstream"
    )
    assert "SECRET" not in redacted_uri
    assert "db.internal:5432/worldstream" in redacted_uri
    assert runner.outcomes[0].status == "PASS"


def test_local_postgresql_reads_owner_only_dsn_file_and_scopes_password(
    tmp_path, monkeypatch, capsys
):
    gates = load_gates()
    dsn_file = tmp_path / "postgres.dsn"
    dsn_file.write_text(
        "host=db.internal port=5432 dbname=worldstream user=operator password=SECRET\n",
        encoding="utf-8",
    )
    dsn_file.chmod(0o600)
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_DSN_FILE", str(dsn_file))
    monkeypatch.delenv("WORLDSTREAM_POSTGRES_URL", raising=False)
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/psql")
    observed = {}

    def fake_run(argv, **kwargs):
        observed["argv"] = argv
        observed["env"] = kwargs["env"]
        return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")

    monkeypatch.setattr(gates, "_run_process_tree", fake_run)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    assert all("SECRET" not in argument for argument in observed["argv"])
    assert observed["env"]["PGPASSWORD"] == "SECRET"
    output = capsys.readouterr().out
    assert "SECRET" not in output
    assert "owner-only DSN file" in output
    assert runner.outcomes[0].status == "PASS"


def test_local_postgresql_rejects_group_readable_dsn_file(tmp_path, monkeypatch):
    if sys.platform == "win32":
        return
    gates = load_gates()
    dsn_file = tmp_path / "postgres.dsn"
    dsn_file.write_text("host=db.invalid dbname=worldstream\n", encoding="utf-8")
    dsn_file.chmod(0o640)
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_DSN_FILE", str(dsn_file))
    monkeypatch.delenv("WORLDSTREAM_POSTGRES_URL", raising=False)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    assert runner.outcomes[0].status == "FAIL"
    assert "owner-only" in runner.outcomes[0].detail


def test_root_python_gate_uses_explicit_sorted_test_inventory(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/uv")
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    commands = []
    monkeypatch.setattr(
        runner,
        "command",
        lambda name, argv, **_kwargs: commands.append((name, argv)) or True,
    )

    gates.root_python_checks(runner)

    test_command = next(argv for name, argv in commands if name == "root-python-tests")
    assert test_command[:7] == [
        "/usr/bin/uv",
        "run",
        "--project",
        "sdk/python",
        "--locked",
        "python",
        "-m",
    ]
    assert "tests/gate_smoke.py" in test_command
    assert test_command[-len(gates.ROOT_PYTHON_TESTS) :] == list(
        gates.ROOT_PYTHON_TESTS
    )


@pytest.mark.parametrize(
    ("tier", "cell_id", "expected"),
    [
        ("pre-push", None, True),
        ("minimal-ci", "native-linux-x86_64", True),
        ("minimal-ci", "native-windows-x64", True),
        ("minimal-ci", "oci-linux-amd64", False),
        ("fast", None, False),
    ],
)
def test_root_python_suite_is_owned_by_pre_push_and_both_native_ci_cells(
    tier, cell_id, expected
):
    gates = load_gates()
    row = None if cell_id is None else {"id": cell_id}

    assert gates.root_python_suite_required(tier, row) is expected


def test_sdk_gate_scopes_pytest_to_the_sdk_tree(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda name: f"/usr/bin/{name}")
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    commands = []
    monkeypatch.setattr(
        runner,
        "command",
        lambda name, argv, **_kwargs: commands.append((name, argv)) or True,
    )

    gates.sdk_and_ui_checks(runner)

    test_command = next(argv for name, argv in commands if name == "python-sdk-tests")
    assert test_command[-2:] == ["pytest", "sdk/python/tests"]


def test_telemetry_pressure_accepts_explicit_rate_limit_backpressure():
    script = (ROOT / "scripts/telemetry-failure-smoke.sh").read_text(encoding="utf-8")

    assert "status in {400, 403, 429}" in script


def test_posix_gate_launcher_falls_back_to_pinned_uv_python():
    script = (ROOT / "scripts/gates.sh").read_text(encoding="utf-8")
    assert "uv run --python 3.14.7 --no-project python" in script
    assert "sys.version_info[:3] == (3, 14, 7)" in script
    checked = subprocess.run(
        ["bash", "-n", str(ROOT / "scripts/gates.sh")],
        check=False,
        capture_output=True,
        text=True,
    )
    assert checked.returncode == 0, checked.stderr


def test_hosted_scanner_inventory_is_version_and_byte_pinned():
    gates = load_gates()
    linux = (ROOT / "scripts/gates-install-tools.sh").read_text(encoding="utf-8")
    windows = (ROOT / "scripts/gates-install-tools.ps1").read_text(encoding="utf-8")
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")

    assert gates.HOSTED_REQUIRED_TOOL_VERSIONS == {
        "cargo-audit": "0.22.2",
        "gitleaks": "8.29.1",
    }
    for version in gates.HOSTED_REQUIRED_TOOL_VERSIONS.values():
        assert version in linux
        assert version in windows
    assert "e4eb209d04e20339d77122a3bdf9cd41351255cfb27ebcb75e85325e04f88924" in linux
    assert "ab28a1bdb54db4d5d8ad5981cf1f959410370b3d28250dbd35f6a44248620e39" in linux
    assert "e4b7d556f0cddbe23d10d8fac2ab0f29f68f019091c6599ffbeaa8a4fb71ac78" in windows
    assert "0a7316540862c13d954f648917ceacca593747baed6eec180fafa590be2710ab" in windows
    linux_prime = linux.index(
        "printf '%s\\n' 'version = 3' | \"$install_dir/cargo-audit\" audit --file -"
    )
    windows_prime = windows.index(
        "'version = 3' | & (Join-Path $InstallDir 'cargo-audit.exe') audit --file -"
    )
    assert linux_prime < linux.index("GITHUB_PATH")
    assert windows_prime < windows.index("$InstallDir | Out-File")
    assert "$LASTEXITCODE -ne 0" in windows[windows_prime:]
    assert "cargo-audit advisory database prime failed" in windows[windows_prime:]
    assert workflow.count("scripts/gates-install-tools.sh") >= 3
    assert "scripts/gates-install-tools.ps1" in workflow


def test_secret_scan_includes_untracked_candidate_files(tmp_path, monkeypatch):
    gates = load_gates()
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    (tmp_path / "tracked.txt").write_text("ordinary source\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tracked.txt"], check=True)
    credential = "postgresql" + "://operator:" + "candidate@db.invalid/example\n"
    (tmp_path / "candidate.txt").write_text(credential, encoding="utf-8")
    monkeypatch.setattr(gates, "ROOT", tmp_path)
    monkeypatch.setattr(gates.shutil, "which", lambda _name: None)
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.secret_scan(runner)

    secret = next(
        outcome for outcome in runner.outcomes if outcome.name == "secret-scan"
    )
    assert secret.status == "FAIL"
    assert "candidate.txt" in secret.detail


def test_detached_release_identity_requires_explicit_locations():
    gates = load_gates()
    manifest = {
        "release_artifacts": [
            detached_artifact_row(gates, artifact_id, profile)
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    assert gates.uses_detached_release_identity(manifest)
    failures, incompletes = gates.release_artifact_inventory_diagnostics(
        manifest, release=True
    )
    assert failures == []
    assert incompletes == []

    manifest["release_artifacts"][0]["digest_location"] = "compatibility.toml"
    assert not gates.uses_detached_release_identity(manifest)
    failures, _ = gates.release_artifact_inventory_diagnostics(manifest, release=True)
    assert any(
        "detached release artifact identity is not enabled" in error
        for error in failures
    )


def test_detached_inventory_binds_exact_artifact_and_evidence_maps(tmp_path):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    artifact_ids = set(gates.RELEASE_ARTIFACT_PROFILES)
    metadata = {
        "schema": gates.DETACHED_RELEASE_MANIFEST_SCHEMA,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {
            artifact_id: f"artifacts/{artifact_id}.bin" for artifact_id in artifact_ids
        },
        "artifact_digests": {
            artifact_id: "sha256:" + "1" * 64
            for artifact_id in artifact_ids
            if artifact_id != "sigstore-bundle"
        },
        "evidence": {"evidence-a": "evidence/evidence-a.json"},
        "evidence_digests": {"evidence-a": "sha256:" + "2" * 64},
        "verification_material": {
            "sigstore-bundle": {"path": "artifacts/sigstore-bundle.bin"}
        },
    }
    manifest = {
        "release_candidate": "0.1.0",
        "release_artifacts": [
            detached_artifact_row(gates, artifact_id, profile)
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    assert gates.detached_release_inventory(runner, metadata, manifest) is not None
    assert runner.outcomes[-1].name == "release-detached-inventory"
    assert runner.outcomes[-1].status == "PASS"

    metadata["evidence_digests"]["unexpected"] = "sha256:" + "3" * 64
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        outcome.name == "release-detached-inventory" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )


def test_detached_cli_first_inventory_is_explicit_and_excludes_only_studio():
    gates = load_gates()
    profile = gates.INVENTORY.CLI_FIRST
    artifact_ids = set(profile.release_artifact_ids)
    metadata = {
        "schema": profile.manifest_schema,
        "release_inventory": profile.identity,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {
            artifact_id: f"artifacts/{artifact_id}.bin" for artifact_id in artifact_ids
        },
        "artifact_digests": {
            artifact_id: "sha256:" + "1" * 64
            for artifact_id in artifact_ids
            if artifact_id != "sigstore-bundle"
        },
        "evidence": {"evidence-a": "evidence/evidence-a.json"},
        "evidence_digests": {"evidence-a": "sha256:" + "2" * 64},
        "verification_material": {
            "sigstore-bundle": {"path": "artifacts/sigstore-bundle.bin"}
        },
    }
    manifest = {
        "release_candidate": "0.1.0",
        "evidence": [{"id": "evidence-a", "release_gate": True}],
    }

    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is not None
    assert "worldstream-participant-console" in metadata["artifacts"]
    assert "worldstream-studio" not in metadata["artifacts"]

    missing_discriminator = dict(metadata)
    missing_discriminator.pop("release_inventory")
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert (
        gates.detached_release_inventory(runner, missing_discriminator, manifest)
        is None
    )

    cross_version = dict(metadata)
    cross_version["schema"] = gates.INVENTORY.LEGACY.manifest_schema
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, cross_version, manifest) is None


def test_spdx_subject_identity_requires_exact_detached_subjects():
    gates = load_gates()
    subjects = {
        "worldstream-0.1.0-source.tar.gz": "sha256:" + "a" * 64,
        "evidence/evidence-a.json": "sha256:" + "b" * 64,
    }
    valid = {
        "files": [
            {
                "fileName": path,
                "checksums": [
                    {
                        "algorithm": "SHA256",
                        "checksumValue": digest.removeprefix("sha256:"),
                    }
                ],
            }
            for path, digest in subjects.items()
        ]
    }
    assert gates.spdx_subject_identity_error(valid, subjects) is None
    valid["files"][0]["checksums"][0]["checksumValue"] = "0" * 64
    assert "mismatch" in gates.spdx_subject_identity_error(valid, subjects)


def test_sigstore_is_verification_material_not_a_signed_subject():
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    artifact_ids = set(gates.RELEASE_ARTIFACT_PROFILES)
    metadata = {
        "schema": gates.DETACHED_RELEASE_MANIFEST_SCHEMA,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {
            artifact_id: f"artifacts/{artifact_id}.bin" for artifact_id in artifact_ids
        },
        "artifact_digests": {
            artifact_id: "sha256:" + "1" * 64
            for artifact_id in artifact_ids
            if artifact_id != "sigstore-bundle"
        },
        "evidence": {"evidence-a": "evidence/evidence-a.json"},
        "evidence_digests": {"evidence-a": "sha256:" + "3" * 64},
        "verification_material": {
            "sigstore-bundle": {"path": "artifacts/sigstore-bundle.bin"}
        },
    }
    manifest = {
        "release_candidate": "0.1.0",
        "release_artifacts": [
            detached_artifact_row(gates, artifact_id, profile)
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    result = gates.detached_release_inventory(runner, metadata, manifest)
    assert result is not None
    assert "sigstore-bundle" not in result[1]
    assert runner.outcomes[-1].status == "PASS"

    metadata["artifact_digests"]["sigstore-bundle"] = "sha256:" + "2" * 64
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        "Sigstore bundle must be path-only verification material" in outcome.detail
        for outcome in runner.outcomes
    )

    metadata["artifact_digests"].pop("sigstore-bundle")
    metadata["verification_material"] = {
        "sigstore-bundle": {"digest": "sha256:" + "4" * 64}
    }
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        "Sigstore bundle must be path-only verification material" in outcome.detail
        for outcome in runner.outcomes
    )
