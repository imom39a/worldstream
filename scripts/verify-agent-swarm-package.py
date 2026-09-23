#!/usr/bin/env python3
"""Run an extracted Agent Swarm archive from an unrelated working directory."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from blake3 import blake3

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_packager", ROOT / "scripts/agent-swarm-package.py"
)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("Agent Swarm packager is unavailable")
PACKAGER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PACKAGER
SPEC.loader.exec_module(PACKAGER)


def tree_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
        if path.is_file():
            digest.update(path.relative_to(root).as_posix().encode())
            digest.update(path.read_bytes())
    return digest.hexdigest()


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        input=input_text,
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
    )
    if result.returncode:
        diagnostic = result.stderr.strip().splitlines()[-1:] or ["no diagnostic"]
        raise RuntimeError(f"{Path(command[0]).name} failed: {diagnostic[0]}")
    return result


def extract_application(
    members: dict[str, tuple[bytes, int]], destination: Path
) -> Path:
    """Extract already-verified bounded members into one application root."""

    destination.mkdir()
    for name, (content, mode) in members.items():
        member = destination / name
        member.parent.mkdir(parents=True, exist_ok=True)
        member.write_bytes(content)
        member.chmod(0o755 if mode & stat.S_IXUSR else 0o644)
    roots = list(destination.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise RuntimeError("archive extraction did not produce one application root")
    return roots[0]


def assert_recovery_required(status: object, swarm_id: str) -> None:
    """Validate the fail-closed status emitted after an unclean update handoff."""

    if not isinstance(status, dict) or status.get("kind") != "status":
        raise RuntimeError("updated daemon returned an invalid status receipt")
    value = status.get("value")
    swarms = value.get("swarms") if isinstance(value, dict) else None
    matching = [
        item
        for item in swarms or []
        if isinstance(item, dict) and item.get("swarm_id") == swarm_id
    ]
    if len(matching) != 1 or matching[0].get("phase") != "recovery_required":
        raise RuntimeError(
            "updated daemon did not remain suspended for explicit Resume"
        )


def assert_distinct_application_versions(
    previous_install: dict[str, object], current_install: dict[str, object]
) -> None:
    """Reject a claimed update when both verified archives are one version."""

    if previous_install.get("application_version") == current_install.get(
        "application_version"
    ):
        raise RuntimeError("previous archive must have a distinct application version")


def exercise_packaged_managed_application(
    *,
    app_root: Path,
    install: dict[str, object],
    configuration_root: Path | None = None,
    configuration_install: dict[str, object] | None = None,
    smoke_root: Path,
    working_area: Path,
) -> dict[str, object]:
    """Drive real Room/TUI/artifact flows using only packaged executables/Pack."""

    pack = install.get("pack")
    pack_path = pack.get("path") if isinstance(pack, dict) else None
    if not isinstance(pack_path, str):
        raise TypeError("packaged managed smoke could not resolve its exact Pack")
    exact_pack = app_root / pack_path
    configuration = (configuration_install or install).get("configuration")
    config_path = configuration.get("path") if isinstance(configuration, dict) else None
    if not isinstance(config_path, str):
        raise TypeError("packaged managed smoke could not resolve its configuration")
    exact_configuration = (configuration_root or app_root) / config_path
    smoke_root.mkdir()
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/verify-agent-swarm-managed.py"),
            "--workspace",
            str(ROOT),
            "--root",
            str(smoke_root),
            "--binary-dir",
            str(app_root / "bin"),
            "--pack",
            str(exact_pack),
            "--config",
            str(exact_configuration),
        ],
        cwd=working_area,
        text=True,
        capture_output=True,
        check=False,
        timeout=600,
    )
    if result.returncode:
        diagnostic = result.stderr.strip().splitlines()[-1:] or ["no diagnostic"]
        raise RuntimeError(f"packaged managed smoke failed: {diagnostic[0]}")
    try:
        receipt = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("packaged managed smoke returned invalid JSON") from error
    required = {
        "status": "ok",
        "backend": "managed_local",
        "reopened": True,
        "reviewed_result": True,
        "artifact_resolved": True,
        "automatic_progress_review": True,
        "automatic_progress_review_waited_for_capacity": True,
        "coordinator_worker_contribution": True,
        "guarded_report_check": True,
        "reviewed_code_change": True,
        "code_change_conflict_preserved": True,
        "room_code_change_result": True,
        "native_shared_capacity": True,
        "native_progress_review_priority": True,
        "native_budget_pause": True,
        "native_effect_recovery": True,
    }
    if not isinstance(receipt, dict) or any(
        receipt.get(key) != expected for key, expected in required.items()
    ):
        raise RuntimeError("packaged managed Room/TUI/artifact smoke was incomplete")
    if not (smoke_root / "managed-processes-stopped").is_file():
        raise RuntimeError("packaged managed smoke did not prove process cleanup")
    return receipt


def reopen_packaged_managed_application(
    *,
    app_root: Path,
    install: dict[str, object],
    configuration_root: Path | None = None,
    configuration_install: dict[str, object] | None = None,
    smoke_root: Path,
    working_area: Path,
) -> dict[str, object]:
    """Use the updated bundle to reopen state produced by the old bundle."""

    configuration = (configuration_install or install).get("configuration")
    config_path = configuration.get("path") if isinstance(configuration, dict) else None
    if not isinstance(config_path, str):
        raise TypeError("updated managed smoke could not resolve its configuration")
    exact_configuration = (configuration_root or app_root) / config_path
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts/verify-agent-swarm-managed.py"),
            "--workspace",
            str(ROOT),
            "--root",
            str(smoke_root),
            "--binary-dir",
            str(app_root / "bin"),
            "--config",
            str(exact_configuration),
            "--reopen-existing",
        ],
        cwd=working_area,
        text=True,
        capture_output=True,
        check=False,
        timeout=300,
    )
    if result.returncode:
        diagnostic = result.stderr.strip().splitlines()[-1:] or ["no diagnostic"]
        raise RuntimeError(f"updated managed reopen failed: {diagnostic[0]}")
    try:
        receipt = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("updated managed reopen returned invalid JSON") from error
    required = {
        "status": "ok",
        "accepted_results_preserved": True,
        "contributions_preserved": True,
        "artifact_preserved": True,
        "provider_selections_preserved": True,
        "tui_restored": True,
    }
    if not isinstance(receipt, dict) or any(
        receipt.get(key) != expected for key, expected in required.items()
    ):
        raise RuntimeError("updated managed Room/Result/artifact reopen was incomplete")
    if not (smoke_root / "managed-update-reopened").is_file():
        raise RuntimeError("updated managed reopen did not retain its proof marker")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument(
        "--previous-archive",
        type=Path,
        help="verified older application archive used for a real update handoff",
    )
    parser.add_argument("--target", choices=sorted(PACKAGER.TARGETS), required=True)
    args = parser.parse_args()
    install, members = PACKAGER._read_verified_archive(
        args.archive,
        target_name=args.target,
    )
    previous_install = None
    previous_members = members
    if args.previous_archive is not None:
        previous_install, previous_members = PACKAGER._read_verified_archive(
            args.previous_archive,
            target_name=args.target,
            require_configuration=False,
        )
        assert_distinct_application_versions(previous_install, install)
    with tempfile.TemporaryDirectory(prefix="agent-swarm-package-smoke-") as temporary:
        temporary_root = Path(temporary)
        extracted_old = temporary_root / "application-old"
        extracted_update = temporary_root / "application-update"
        unrelated = temporary_root / "unrelated-working-directory"
        state = temporary_root / "state"
        unrelated.mkdir()
        state.mkdir()
        old_root = extract_application(previous_members, extracted_old)
        app_root = extract_application(members, extracted_update)
        old_before = tree_digest(old_root)
        update_before = tree_digest(app_root)
        suffix = ".exe" if args.target == "windows-x64" else ""
        app = app_root / "bin" / f"worldstream-agent-swarm{suffix}"
        worker = app_root / "bin" / f"worldstream-agent-swarm-controlled-worker{suffix}"
        guard = app_root / "bin" / f"worldstream-agent-swarm-process-guard{suffix}"
        daemon = app_root / "bin" / f"worldstream-agent-swarmd{suffix}"
        control = app_root / "bin" / f"worldstream-agent-swarmctl{suffix}"
        run([str(app), "--help"], cwd=unrelated)
        run([str(daemon), "--help"], cwd=unrelated)
        run([str(control), "--help"], cwd=unrelated)
        smoke = run([str(app), "smoke-test", "--state-dir", str(state)], cwd=unrelated)
        smoke_receipt = json.loads(smoke.stdout)
        if smoke_receipt.get("status") != "ok":
            raise RuntimeError("packaged fixture smoke did not complete")
        version = run([str(worker), "--version"], cwd=unrelated)
        if not version.stdout.startswith("worldstream-controlled-worker "):
            raise RuntimeError("packaged controlled worker version is invalid")
        environment = dict(os.environ)
        environment["WORLDSTREAM_CONTROLLED_PROVIDER"] = "1"
        invoked = run(
            [
                str(worker),
                "invoke",
                "--invocation-id",
                "package-smoke-invocation",
                "--model",
                "controlled-model-a",
                "--effort",
                "high",
                "--behavior",
                "normal",
                "--new-session",
                "package-smoke-session",
            ],
            cwd=unrelated,
            env=environment,
            input_text="packaged controlled worker prompt",
        )
        events = [json.loads(line) for line in invoked.stdout.splitlines() if line]
        if events != [
            {
                "effort": "high",
                "model": "controlled-model-a",
                "session_id": "package-smoke-session",
                "type": "configuration",
            },
            {
                "invocation_id": "package-smoke-invocation",
                "text": "packaged controlled worker prompt",
                "type": "result",
            },
        ]:
            raise RuntimeError("packaged controlled worker contract mismatch")
        exercise_update_daemon(
            old_daemon=old_root / "bin" / f"worldstream-agent-swarmd{suffix}",
            old_control=old_root / "bin" / f"worldstream-agent-swarmctl{suffix}",
            old_guard=old_root
            / "bin"
            / f"worldstream-agent-swarm-process-guard{suffix}",
            daemon=daemon,
            control=control,
            guard=guard,
            worker=worker,
            state=state / "execution",
            working_area=unrelated,
        )
        managed_smoke_root = temporary_root / "packaged-managed-smoke"
        managed_install = previous_install or install
        managed_configuration = managed_install.get("configuration")
        configuration_root = old_root
        configuration_install = managed_install
        if not isinstance(managed_configuration, dict):
            configuration_root = app_root
            configuration_install = install
        managed_receipt = exercise_packaged_managed_application(
            app_root=old_root,
            install=managed_install,
            configuration_root=configuration_root,
            configuration_install=configuration_install,
            smoke_root=managed_smoke_root,
            working_area=unrelated,
        )
        update_managed_receipt = reopen_packaged_managed_application(
            app_root=app_root,
            install=install,
            configuration_root=configuration_root,
            configuration_install=configuration_install,
            smoke_root=managed_smoke_root,
            working_area=unrelated,
        )
        if (
            tree_digest(old_root) != old_before
            or tree_digest(app_root) != update_before
        ):
            raise RuntimeError("packaged execution mutated an application directory")
    print(
        json.dumps(
            {
                "status": "ok",
                "target": args.target,
                "application_version": install["application_version"],
                "pack_digest": install["pack"]["digest"],
                "independent_working_directory": True,
                "application_directory_immutable": True,
                "update_verification": (
                    "side_by_side_update"
                    if previous_install is not None
                    else "reinstall_only_update_not_exercised"
                ),
                "previous_application_version": (
                    previous_install["application_version"]
                    if previous_install is not None
                    else None
                ),
                "external_state_preserved": True,
                "suspended_until_explicit_resume": True,
                "controlled_worker": True,
                "daemon_guarded_worker": True,
                "managed_rooms": managed_receipt["rooms"],
                "packaged_room_tui_artifact": True,
                "managed_update_reopened": True,
                "managed_update_swarm_id": update_managed_receipt["swarm_id"],
            },
            sort_keys=True,
            separators=(",", ":"),
        )
    )
    return 0


def exercise_update_daemon(
    *,
    old_daemon: Path,
    old_control: Path,
    old_guard: Path,
    daemon: Path,
    control: Path,
    guard: Path,
    worker: Path,
    state: Path,
    working_area: Path,
) -> None:
    """Prove an adjacent update recovers shared state fail-closed, then runs."""

    invocation_id = "package-daemon-invocation"
    swarm_id = "package-daemon-swarm"
    member_id = "package-daemon-member"
    ticket = {
        "invocation_id": invocation_id,
        "swarm_id": swarm_id,
        "member_id": member_id,
        "provider": "controlled",
        "configuration_revision": 1,
        "kind": "work",
        "due_sequence": 1,
    }
    prepared = {
        "provider": "controlled",
        "invocation_id": invocation_id,
        "member_id": member_id,
        "configuration_revision": 1,
        "program": str(worker.resolve()),
        "executable_digest": f"blake3:{blake3(worker.read_bytes()).hexdigest()}",
        "qualification": None,
        "arguments": [
            "invoke",
            "--invocation-id",
            invocation_id,
            "--model",
            "controlled-model-daemon",
            "--effort",
            "high",
            "--behavior",
            "normal",
            "--new-session",
            "package-daemon-session",
        ],
        "working_area": str(working_area.resolve()),
        "environment_remove": [],
        "environment_set": {"WORLDSTREAM_CONTROLLED_PROVIDER": "1"},
        "stdin": "packaged daemon guarded prompt",
        "requested_model": "controlled-model-daemon",
        "requested_effort": "high",
        "requested_session": {
            "mode": "fresh",
            "requested_id": "package-daemon-session",
        },
        "resolution": {
            "state": "verified",
            "model": "controlled-model-daemon",
            "effort": "high",
        },
        "output_contract": "controlled_json_lines",
    }
    ticket_path = state.parent / "ticket.json"
    prepared_path = state.parent / "prepared.json"
    ticket_path.write_text(json.dumps(ticket), encoding="utf-8")
    prepared_path.write_text(json.dumps(prepared), encoding="utf-8")
    old_process = subprocess.Popen(
        [str(old_daemon), "--state", str(state), "--process-guard", str(old_guard)],
        cwd=working_area,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        publication = state / "execution-control.v1.json"
        deadline = time.monotonic() + 10
        while not publication.is_file():
            if old_process.poll() is not None:
                _, stderr = old_process.communicate(timeout=1)
                raise RuntimeError(
                    f"packaged execution daemon exited early: {stderr.strip()}"
                )
            if time.monotonic() >= deadline:
                raise RuntimeError(
                    "packaged execution daemon did not publish readiness"
                )
            time.sleep(0.05)

        old_prefix = [str(old_control), "--state", str(state)]
        run([*old_prefix, "register", swarm_id], cwd=working_area)
        run(
            [*old_prefix, "set-provider-cap", "controlled", "1"],
            cwd=working_area,
        )
        run([*old_prefix, "resume", swarm_id], cwd=working_area)
    finally:
        if old_process.poll() is None:
            old_process.terminate()
            try:
                old_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                old_process.kill()
                old_process.wait(timeout=5)

    process = subprocess.Popen(
        [str(daemon), "--state", str(state), "--process-guard", str(guard)],
        cwd=working_area,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        publication = state / "execution-control.v1.json"
        deadline = time.monotonic() + 10
        while not publication.is_file():
            if process.poll() is not None:
                _, stderr = process.communicate(timeout=1)
                raise RuntimeError(
                    f"updated execution daemon exited early: {stderr.strip()}"
                )
            if time.monotonic() >= deadline:
                raise RuntimeError("updated execution daemon did not publish readiness")
            time.sleep(0.05)

        prefix = [str(control), "--state", str(state)]
        status_result = None
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                status_result = run(
                    [*prefix, "status", "--swarm-id", swarm_id], cwd=working_area
                )
                break
            except RuntimeError:
                if process.poll() is not None:
                    _, stderr = process.communicate(timeout=1)
                    raise RuntimeError(
                        f"updated execution daemon exited early: {stderr.strip()}"
                    ) from None
                time.sleep(0.05)
        if status_result is None:
            raise RuntimeError(
                "updated execution daemon control endpoint was not ready"
            )
        status = json.loads(status_result.stdout)
        assert_recovery_required(status, swarm_id)
        run([*prefix, "enqueue", "--ticket", str(ticket_path)], cwd=working_area)
        suspended = json.loads(run([*prefix, "tick"], cwd=working_area).stdout)
        if suspended.get("value", {}).get("admissions"):
            raise RuntimeError("updated daemon admitted work before explicit Resume")
        run([*prefix, "resume", swarm_id], cwd=working_area)
        admitted = json.loads(run([*prefix, "tick"], cwd=working_area).stdout)
        admissions = admitted.get("value", {}).get("admissions", [])
        if len(admissions) != 1 or admissions[0].get("ticket") != ticket:
            raise RuntimeError("packaged daemon did not admit the exact ticket")
        run(
            [
                *prefix,
                "launch",
                "--ticket",
                str(ticket_path),
                "--prepared",
                str(prepared_path),
            ],
            cwd=working_area,
        )

        completion = None
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            tick = json.loads(run([*prefix, "tick"], cwd=working_area).stdout)
            completions = tick.get("value", {}).get("completions", [])
            if any(item.get("invocation_id") == invocation_id for item in completions):
                completion = json.loads(
                    run([*prefix, "collect", invocation_id], cwd=working_area).stdout
                )
                break
            time.sleep(0.05)
        if completion is None:
            raise RuntimeError("packaged daemon did not retain worker completion")
        value = completion.get("value", {})
        exit_value = value.get("exit")
        if value.get("resolution") != "completed" or not isinstance(exit_value, dict):
            raise RuntimeError("packaged daemon retained an invalid completion")
        stdout = bytes(exit_value.get("stdout", [])).decode("utf-8")
        events = [json.loads(line) for line in stdout.splitlines() if line]
        if events[-1].get("text") != "packaged daemon guarded prompt":
            raise RuntimeError("guarded worker output was not retained exactly")
        run(
            [*prefix, "acknowledge-completion", invocation_id],
            cwd=working_area,
        )
        run([*prefix, "pause", swarm_id], cwd=working_area)
        run([*prefix, "stop", swarm_id], cwd=working_area)
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())
