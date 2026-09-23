"""Fail-closed mixed-provider qualification evidence tests."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_qualification",
    ROOT / "scripts/agent-swarm-qualification.py",
)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("could not load Agent Swarm qualification assembler")
QUALIFICATION = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = QUALIFICATION
SPEC.loader.exec_module(QUALIFICATION)

REVISION = "f" * 40
PACK_DIGEST = "blake3:" + "e" * 64
TARGETS = {
    "macos-arm64": ("macos", "arm64"),
    "macos-x86_64": ("macos", "x86_64"),
    "windows-x64": ("windows", "x86_64"),
}
PACKAGE_DIGESTS = {
    "macos-arm64": "a" * 64,
    "macos-x86_64": "9" * 64,
    "windows-x64": "8" * 64,
}
PROVIDER_LAYOUT = {
    "codex": (4, 4),
    "claude": (3, 3),
    "kiro": (3, 3),
}


def canonical(value: object) -> bytes:
    return QUALIFICATION.canonical_json(value)


def install_receipt(
    target: str, application_version: str = "0.1.0"
) -> dict[str, object]:
    return {
        "schema": QUALIFICATION.INSTALL_SCHEMA,
        "application_version": application_version,
        "target": target,
        "execution_default": "suspended_until_explicit_resume",
        "provider_management": "discover_only_never_install_or_reconfigure",
        "state_location": "platform_user_data/worldstream/agent-swarm",
        "pack": {
            "id": QUALIFICATION.PACK_ID,
            "version": "0.2.0",
            "digest": PACK_DIGEST,
            "path": "packs/worldstream.agent-swarm/exact/agent-swarm.wspack",
        },
        "files": {"bin/worldstream-agent-swarm": "1" * 64},
    }


def bound_receipt(
    schema: str,
    target: str,
    install_sha256: str,
    **payload: object,
) -> dict[str, object]:
    return {
        "schema": schema,
        "run_id": f"native-{target}-run",
        "target": target,
        "package_sha256": PACKAGE_DIGESTS[target],
        "install_sha256": install_sha256,
        "engine_instance_id": f"engine-{target}",
        **payload,
    }


def invocation_receipt(
    *,
    target: str,
    install_sha256: str,
    provider: str,
    member_key: str,
    invocation_id: str,
    revision: int,
    started_at_ms: int,
    finished_at_ms: int,
    model: str,
    effort: str,
    session_mode: str,
    session_id: str,
    submitted: bool = True,
) -> dict[str, object]:
    return bound_receipt(
        QUALIFICATION.PROVIDER_INVOCATION_SCHEMA,
        target,
        install_sha256,
        provider=provider,
        cli_version=f"{provider}-fixture-version",
        executable_blake3="blake3:" + "d" * 64,
        member_key=member_key,
        invocation_id=invocation_id,
        configuration_revision=revision,
        started_at_ms=started_at_ms,
        finished_at_ms=finished_at_ms,
        requested={
            "model": model,
            "effort": effort,
            "session": {"mode": session_mode, "session_id": session_id},
        },
        reported=(
            {"model": model, "effort": effort, "session_id": session_id}
            if submitted
            else None
        ),
        proposal_schema=(
            "worldstream/agent-swarm-action-proposal@1" if submitted else None
        ),
        coordinator_disposition="submitted" if submitted else "not_submitted",
        submitted_action_id=f"action-{invocation_id}" if submitted else None,
        blocker=None if submitted else "unavailable_setting",
    )


def provider_receipts(
    target: str, install_sha256: str, provider: str, member_count: int, cap: int
) -> tuple[dict[str, dict[str, object]], dict[str, object], list[dict[str, object]]]:
    documents: dict[str, dict[str, object]] = {}
    accepted: list[dict[str, object]] = []
    refs: list[str] = []
    for index in range(member_count):
        model, effort = (
            (f"{provider}-model-a", "low")
            if index % 2 == 0
            else (f"{provider}-model-b", "high")
        )
        started_at = 100 if index < 2 else 230 + (index - 2) * 50
        finished_at = 200 if index < 2 else started_at + 30
        receipt_id = f"{provider}-invocation-{index}"
        receipt = invocation_receipt(
            target=target,
            install_sha256=install_sha256,
            provider=provider,
            member_key=f"{provider}-member-{index}",
            invocation_id=f"{provider}-inv-{index}",
            revision=1,
            started_at_ms=started_at,
            finished_at_ms=finished_at,
            model=model,
            effort=effort,
            session_mode="fresh",
            session_id=f"{provider}-session-{index}",
        )
        documents[receipt_id] = receipt
        accepted.append(receipt)
        refs.append(receipt_id)

    next_turn_id = f"{provider}-invocation-next-turn"
    next_turn = invocation_receipt(
        target=target,
        install_sha256=install_sha256,
        provider=provider,
        member_key=f"{provider}-member-0",
        invocation_id=f"{provider}-inv-next-turn",
        revision=2,
        started_at_ms=400,
        finished_at_ms=450,
        model=f"{provider}-model-a",
        effort="low",
        session_mode="resume",
        session_id=f"{provider}-session-0",
    )
    documents[next_turn_id] = next_turn
    accepted.append(next_turn)
    refs.append(next_turn_id)

    changed_turn_id = f"{provider}-invocation-changed-setting"
    changed_turn = invocation_receipt(
        target=target,
        install_sha256=install_sha256,
        provider=provider,
        member_key=f"{provider}-member-0",
        invocation_id=f"{provider}-inv-changed-setting",
        revision=3,
        started_at_ms=460,
        finished_at_ms=490,
        model=f"{provider}-model-b",
        effort="high",
        session_mode="fresh",
        session_id=f"{provider}-session-changed-setting",
    )
    documents[changed_turn_id] = changed_turn
    accepted.append(changed_turn)
    refs.append(changed_turn_id)

    blocked_id = f"{provider}-invocation-blocked"
    blocked = invocation_receipt(
        target=target,
        install_sha256=install_sha256,
        provider=provider,
        member_key=f"{provider}-member-{member_count - 1}",
        invocation_id=f"{provider}-inv-blocked",
        revision=2,
        started_at_ms=510,
        finished_at_ms=520,
        model=f"{provider}-unavailable-model",
        effort="maximum",
        session_mode="fresh",
        session_id=f"{provider}-blocked-session",
        submitted=False,
    )
    documents[blocked_id] = blocked
    refs.append(blocked_id)

    login_id = f"{provider}-login"
    documents[login_id] = bound_receipt(
        QUALIFICATION.PROVIDER_LOGIN_SCHEMA,
        target,
        install_sha256,
        provider=provider,
        cli_version=f"{provider}-fixture-version",
        executable_blake3="blake3:" + "d" * 64,
        login_mode="normal_subscription_cli",
        removed_environment=sorted(QUALIFICATION.DIRECT_API_ENVIRONMENT),
        exit_code=0,
        result_sha256="2" * 64,
    )
    refs.append(login_id)

    process_id = f"{provider}-process"
    documents[process_id] = bound_receipt(
        QUALIFICATION.PROCESS_CONTAINMENT_SCHEMA,
        target,
        install_sha256,
        provider=provider,
        invocation_id=f"{provider}-inv-0",
        owned_process_ids=[f"{provider}-parent", f"{provider}-child"],
        delegated_process_ids=[],
        escaped_process_ids=[],
        terminated_process_ids=[f"{provider}-parent", f"{provider}-child"],
        cancel_outcome="terminated",
        unrelated_process_id=f"{provider}-unrelated",
        unrelated_process_status="running",
    )
    refs.append(process_id)

    working_area = (
        "C:/qualification/root"
        if target.startswith("windows-")
        else "/qualification/root"
    )
    escape_path = (
        "C:/qualification/outside/probe.txt"
        if target.startswith("windows-")
        else "/qualification/outside/probe.txt"
    )
    for suffix, model, effort in (
        ("a", f"{provider}-model-a", "low"),
        ("b", f"{provider}-model-b", "high"),
    ):
        confinement_id = f"{provider}-confinement-{suffix}"
        documents[confinement_id] = bound_receipt(
            QUALIFICATION.RESOURCE_CONFINEMENT_SCHEMA,
            target,
            install_sha256,
            provider=provider,
            cli_version=f"{provider}-fixture-version",
            executable_blake3="blake3:" + "d" * 64,
            invocation_id=f"{provider}-confinement-{suffix}",
            selection={"model": model, "effort": effort},
            working_area=working_area,
            resource_policy="workspace_write",
            allowed_tools=[],
            enforcement_layer="worldstream-agent-swarm-process-guard",
            in_root_probe={
                "path": f"{working_area}/inside-{suffix}.txt",
                "outcome": "allowed",
            },
            escape_probe={"path": escape_path, "outcome": "denied"},
            unauthorized_tool_probe={"tool": "shell", "outcome": "denied"},
        )
        refs.append(confinement_id)

    summary = {
        "provider": provider,
        "status": "pass",
        "reason": "qualified with content-addressed native receipts",
        "evidence_refs": refs,
        "cli_version": f"{provider}-fixture-version",
        "executable_blake3": "blake3:" + "d" * 64,
        "explicit_model": True,
        "explicit_effort": True,
        "session_reuse": True,
        "member_count": member_count,
        "effective_cap": cap,
        "maximum_simultaneous_invocations": 2,
        "selections": [
            {"model": f"{provider}-model-a", "effort": "low"},
            {"model": f"{provider}-model-b", "effort": "high"},
        ],
        "assertions": {
            assertion: True for assertion in QUALIFICATION.PROVIDER_ASSERTIONS
        },
    }
    return documents, summary, accepted


def scheduler_event(**overrides: object) -> dict[str, object]:
    value = {
        "sequence": 0,
        "time_ms": 0,
        "round": None,
        "event": "snapshot",
        "swarm_id": None,
        "invocation_id": None,
        "member_key": None,
        "provider": None,
        "kind": None,
        "provider_cap": None,
        "priority": None,
        "priority_source": None,
        "budget_invocation_limit": 20,
        "budget_active_time_limit_ms": 120_000,
        "desired": None,
        "phase": None,
    }
    value.update(overrides)
    return value


def scheduler_receipt(
    target: str,
    install_sha256: str,
    accepted_by_provider: dict[str, list[dict[str, object]]],
) -> dict[str, object]:
    pending: list[tuple[int, int, dict[str, object]]] = []
    order = 0

    def add(time_ms: int, **fields: object) -> None:
        nonlocal order
        pending.append((time_ms, order, scheduler_event(time_ms=time_ms, **fields)))
        order += 1

    for provider, (_, cap) in PROVIDER_LAYOUT.items():
        add(0, event="snapshot", provider=provider, provider_cap=cap)

    codex_zero = accepted_by_provider["codex"][0]
    codex_one = accepted_by_provider["codex"][1]
    claude_zero = accepted_by_provider["claude"][0]
    kiro_zero = accepted_by_provider["kiro"][0]
    add(
        80,
        event="timer_due",
        invocation_id=codex_one["invocation_id"],
        provider="codex",
        kind="progress_review",
        provider_cap=4,
    )
    add(
        81,
        event="review_claimed",
        invocation_id=codex_one["invocation_id"],
        provider="codex",
        kind="progress_review",
        provider_cap=4,
    )
    add(
        90,
        round=1,
        event="eligible",
        swarm_id="swarm-default",
        invocation_id=codex_zero["invocation_id"],
        member_key=codex_zero["member_key"],
        provider="codex",
        kind="work",
        provider_cap=4,
        priority=1,
        priority_source="default",
    )
    add(
        90,
        round=1,
        event="eligible",
        swarm_id="swarm-explicit",
        invocation_id=claude_zero["invocation_id"],
        member_key=claude_zero["member_key"],
        provider="claude",
        kind="work",
        provider_cap=3,
        priority=1,
        priority_source="explicit",
    )
    add(
        91,
        round=2,
        event="eligible",
        swarm_id="swarm-review",
        invocation_id=codex_one["invocation_id"],
        member_key=codex_one["member_key"],
        provider="codex",
        kind="progress_review",
        provider_cap=4,
        priority=1,
        priority_source="default",
    )
    add(
        91,
        round=2,
        event="eligible",
        swarm_id="swarm-work",
        invocation_id=kiro_zero["invocation_id"],
        member_key=kiro_zero["member_key"],
        provider="kiro",
        kind="work",
        provider_cap=3,
        priority=1,
        priority_source="default",
    )

    first_wave = [
        ("codex", accepted_by_provider["codex"][0], 1, "work", "swarm-default"),
        ("claude", accepted_by_provider["claude"][0], 1, "work", "swarm-explicit"),
        (
            "codex",
            accepted_by_provider["codex"][1],
            2,
            "progress_review",
            "swarm-review",
        ),
        ("kiro", accepted_by_provider["kiro"][0], 2, "work", "swarm-work"),
        ("claude", accepted_by_provider["claude"][1], 3, "work", "swarm-claude"),
        ("kiro", accepted_by_provider["kiro"][1], 3, "work", "swarm-kiro"),
    ]
    first_ids = {str(item[1]["invocation_id"]) for item in first_wave}
    for provider, receipt, round_value, kind, swarm_id in first_wave:
        add(
            int(receipt["started_at_ms"]),
            round=round_value,
            event="admitted",
            swarm_id=swarm_id,
            invocation_id=receipt["invocation_id"],
            member_key=receipt["member_key"],
            provider=provider,
            kind=kind,
            provider_cap=PROVIDER_LAYOUT[provider][1],
            priority=1,
            priority_source="default",
        )
    for provider, receipts in accepted_by_provider.items():
        for receipt in receipts:
            if str(receipt["invocation_id"]) not in first_ids:
                add(
                    int(receipt["started_at_ms"]),
                    round=4,
                    event="admitted",
                    swarm_id=f"swarm-{provider}",
                    invocation_id=receipt["invocation_id"],
                    member_key=receipt["member_key"],
                    provider=provider,
                    kind="work",
                    provider_cap=PROVIDER_LAYOUT[provider][1],
                    priority=1,
                    priority_source="default",
                )
            add(
                int(receipt["finished_at_ms"]),
                event="completed",
                swarm_id=f"swarm-{provider}",
                invocation_id=receipt["invocation_id"],
                member_key=receipt["member_key"],
                provider=provider,
                kind=(
                    "progress_review"
                    if receipt["invocation_id"] == codex_one["invocation_id"]
                    else "work"
                ),
                provider_cap=PROVIDER_LAYOUT[provider][1],
                priority=1,
                priority_source="default",
            )
    add(
        600,
        event="budget_threshold",
        swarm_id="swarm-budget",
        priority=1,
        priority_source="explicit",
    )
    add(
        601,
        event="desired_changed",
        swarm_id="swarm-budget",
        desired="paused",
        phase="paused",
    )
    events = []
    for sequence, (_, _, event) in enumerate(sorted(pending), start=1):
        event["sequence"] = sequence
        events.append(event)
    return bound_receipt(
        QUALIFICATION.SCHEDULER_TRACE_SCHEMA,
        target,
        install_sha256,
        machine_invocation_limit=8,
        progress_review_interval_seconds=5,
        slow_worker_delay_ms=10_000,
        events=events,
    )


def result_lineage(kind: str) -> dict[str, object]:
    artifact_sha256 = "b" * 64 if kind == "report" else "c" * 64
    return {
        "accepted_result_id": f"result-{kind}-v1",
        "candidate_id": f"candidate-{kind}-v1",
        "review_id": f"review-{kind}-v1",
        "producer_member_key": "producer-a",
        "reviewer_member_key": "reviewer-b",
        "artifact_sha256": artifact_sha256,
    }


def scenario_summary(name: str, target: str, receipt_id: str) -> dict[str, object]:
    lineage = None
    if name == "sourced_report":
        lineage = result_lineage("report")
    elif name == "reviewed_code_change":
        lineage = result_lineage("code")
    return {
        "verdict": "pass",
        "reason": f"retained native evidence for {name}",
        "evidence_refs": [receipt_id],
        "engine_instance_id": f"engine-{target}",
        "goal_ref": f"goal:{name}",
        "room_id": f"room-{target}-{name}",
        "swarm_id": f"swarm-{target}-{name}",
        "criteria_revision": 1,
        "head": {
            "room_seq": 42,
            "authoritative_state_hash": "blake3:" + "a" * 64,
        },
        "action_id": f"action-{name}",
        "result_lineage": lineage,
    }


def evidence(target: str) -> dict[str, object]:
    operating_system, architecture = TARGETS[target]
    install = install_receipt(target)
    install_sha256 = hashlib.sha256(canonical(install)).hexdigest()
    documents: dict[str, dict[str, object]] = {"install": install}
    providers = []
    accepted_by_provider: dict[str, list[dict[str, object]]] = {}
    for provider, (member_count, cap) in PROVIDER_LAYOUT.items():
        provider_documents, summary, accepted = provider_receipts(
            target, install_sha256, provider, member_count, cap
        )
        documents.update(provider_documents)
        providers.append(summary)
        accepted_by_provider[provider] = accepted
    documents["scheduler"] = scheduler_receipt(
        target, install_sha256, accepted_by_provider
    )

    scenarios = {}
    for name in QUALIFICATION.SCENARIOS:
        receipt_id = f"scenario-{name}"
        summary = scenario_summary(name, target, receipt_id)
        if name in {
            "sourced_report",
            "reviewed_code_change",
            "explicit_provider_settings",
            "unavailable_settings_blocked",
        }:
            source_receipt_ids = ["codex-invocation-0"]
        elif name == "owned_descendant_cleanup":
            source_receipt_ids = ["codex-process"]
        else:
            source_receipt_ids = ["scheduler"]
        documents[receipt_id] = bound_receipt(
            QUALIFICATION.SCENARIO_RECEIPT_SCHEMA,
            target,
            install_sha256,
            scenario=name,
            verdict=summary["verdict"],
            reason=summary["reason"],
            goal_ref=summary["goal_ref"],
            room_id=summary["room_id"],
            swarm_id=summary["swarm_id"],
            criteria_revision=summary["criteria_revision"],
            head=summary["head"],
            action_id=summary["action_id"],
            result_lineage=summary["result_lineage"],
            source_receipt_ids=source_receipt_ids,
        )
        scenarios[name] = summary

    return {
        "_receipt_documents": documents,
        "schema": QUALIFICATION.EVIDENCE_SCHEMA,
        "run_id": f"native-{target}-run",
        "recorded_at": "2026-09-15T12:00:00Z",
        "git_revision": REVISION,
        "package": {
            "sha256": PACKAGE_DIGESTS[target],
            "target": target,
            "application_version": "0.1.0",
            "install_schema": QUALIFICATION.INSTALL_SCHEMA,
            "install_sha256": install_sha256,
            "install_ref": "install",
            "pack_id": QUALIFICATION.PACK_ID,
            "pack_version": "0.2.0",
            "pack_digest": PACK_DIGEST,
        },
        "platform": {
            "os": operating_system,
            "arch": architecture,
            "native": True,
        },
        "coordination_engine": {
            "application": QUALIFICATION.COORDINATION_ENGINE,
            "instance_id": f"engine-{target}",
            "pack_digest": PACK_DIGEST,
        },
        "roster_size": 10,
        "execution": {
            "machine_invocation_limit": 8,
            "maximum_simultaneous_invocations": 6,
            "priority_policy": {
                "default_priority": 1,
                "explicit_equal_priority": 1,
                "equal_priority_order_observed": True,
                "progress_review_priority_observed": True,
            },
            "budget_policy": {
                "invocation_limit": 20,
                "active_time_limit_ms": 120_000,
                "pause_observed": True,
            },
            "timer_traffic": {
                "progress_review_interval_seconds": 5,
                "slow_worker_delay_ms": 10_000,
                "due_reviews_observed": 1,
            },
            "evidence_refs": ["scheduler"],
        },
        "providers": providers,
        "scenarios": scenarios,
        "artifacts": {
            "report": {
                "sha256": "b" * 64,
                "accepted_result_id": "result-report-v1",
            },
            "code_change": {
                "sha256": "c" * 64,
                "accepted_result_id": "result-code-v1",
            },
        },
        "status": "pass",
    }


def write(path: Path, value: object) -> Path:
    document = copy.deepcopy(value)
    receipts = document.pop("_receipt_documents")
    receipt_directory = path.parent / f"{path.stem}-receipts"
    receipt_directory.mkdir()
    manifest = {}
    for receipt_id, receipt in sorted(receipts.items()):
        receipt_path = receipt_directory / f"{receipt_id}.json"
        raw = canonical(receipt)
        receipt_path.write_bytes(raw)
        schema = receipt["schema"]
        manifest[receipt_id] = {
            "path": receipt_path.relative_to(path.parent).as_posix(),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "schema": schema,
            "kind": QUALIFICATION.RECEIPT_KINDS[schema],
        }
    document["receipt_manifest"] = manifest
    path.write_bytes(canonical(document))
    return path


def complete_paths(tmp_path: Path) -> list[Path]:
    return [write(tmp_path / f"{target}.json", evidence(target)) for target in TARGETS]


def provider_summary(value: dict[str, object], name: str) -> dict[str, object]:
    return next(item for item in value["providers"] if item["provider"] == name)


def set_provider_login_failure(
    value: dict[str, object], name: str, status: str, reason: str
) -> None:
    summary = provider_summary(value, name)
    summary["status"] = status
    summary["reason"] = reason
    summary["assertions"]["subscription_login"] = False
    login = value["_receipt_documents"][f"{name}-login"]
    login["login_mode"] = "failed" if status == "fail" else "unavailable"
    login["exit_code"] = 1
    value["status"] = status


def test_assembles_only_complete_content_addressed_target_matrix(tmp_path: Path):
    paths = complete_paths(tmp_path)
    report = QUALIFICATION.assemble(paths, REVISION)

    assert report["status"] == "pass"
    assert report["missing_targets"] == []
    assert len(report["support_matrix"]) == 9
    assert {row["status"] for row in report["support_matrix"]} == {"pass"}
    assert len(report["scenario_matrix"]) == len(TARGETS) * len(QUALIFICATION.SCENARIOS)
    assert report["qualification_gaps"] == []

    records = QUALIFICATION.provider_qualification_records(paths, REVISION)
    assert len(records) == 9
    codex = records["macos-arm64-codex"]
    assert codex["schema"] == QUALIFICATION.PROVIDER_QUALIFICATION_SCHEMA
    assert codex["executable_digest"] == "blake3:" + "d" * 64
    assert codex["evidence_path"] == "macos-arm64/evidence.json"
    assert len(codex["receipt_manifest_sha256"]) == 64


def test_fresh_provider_generated_session_can_be_resumed(tmp_path: Path):
    value = evidence("macos-arm64")
    first = value["_receipt_documents"]["codex-invocation-0"]
    first["requested"]["session"]["session_id"] = None

    loaded, _ = QUALIFICATION.load_evidence(
        write(tmp_path / "fresh-session.json", value)
    )

    codex = next(item for item in loaded["providers"] if item["provider"] == "codex")
    assert codex["session_reuse"] is True


def test_missing_target_is_retained_as_a_fail_closed_support_gap(tmp_path: Path):
    paths = [
        write(tmp_path / f"{target}.json", evidence(target))
        for target in ("macos-arm64", "windows-x64")
    ]
    report = QUALIFICATION.assemble(paths, REVISION)

    assert report["status"] == "fail"
    assert report["missing_targets"] == ["macos-x86_64"]
    missing = [
        row for row in report["support_matrix"] if row["target"] == "macos-x86_64"
    ]
    assert len(missing) == 3
    assert {row["status"] for row in missing} == {"missing"}
    with pytest.raises(
        QUALIFICATION.QualificationError, match="passing complete target matrix"
    ):
        QUALIFICATION.provider_qualification_records(paths, REVISION)


def test_failed_and_unsupported_receipts_remain_in_failure_report(tmp_path: Path):
    macos_arm = evidence("macos-arm64")
    set_provider_login_failure(
        macos_arm, "codex", "fail", "normal subscription login failed"
    )
    macos_intel = evidence("macos-x86_64")
    set_provider_login_failure(
        macos_intel, "kiro", "unsupported", "Kiro CLI login unavailable"
    )
    paths = [
        write(tmp_path / "macos-arm64.json", macos_arm),
        write(tmp_path / "macos-x86_64.json", macos_intel),
        write(tmp_path / "windows-x64.json", evidence("windows-x64")),
    ]
    report = QUALIFICATION.assemble(paths, REVISION)

    assert report["status"] == "fail"
    statuses = {
        (row["target"], row["provider"]): row["status"]
        for row in report["support_matrix"]
    }
    assert statuses[("macos-arm64", "codex")] == "fail"
    assert statuses[("macos-x86_64", "kiro")] == "unsupported"
    assert any(
        "subscription login failed" in gap for gap in report["qualification_gaps"]
    )
    assert any(
        "Kiro CLI login unavailable" in gap for gap in report["qualification_gaps"]
    )


def test_boolean_only_scenario_evidence_is_rejected(tmp_path: Path):
    value = evidence("macos-arm64")
    value["scenarios"] = {name: True for name in QUALIFICATION.SCENARIOS}
    path = write(tmp_path / "boolean-only.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="must contain exactly"):
        QUALIFICATION.load_evidence(path)


def test_unmanifested_and_tampered_receipts_are_rejected(tmp_path: Path):
    value = evidence("macos-arm64")
    value["providers"][0]["evidence_refs"][0] = "opaque:trust-me"
    path = write(tmp_path / "opaque.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="unmanifested"):
        QUALIFICATION.load_evidence(path)

    path = write(tmp_path / "tampered.json", evidence("macos-arm64"))
    document = json.loads(path.read_text(encoding="utf-8"))
    receipt_path = path.parent / document["receipt_manifest"]["scheduler"]["path"]
    receipt_path.write_text('{"schema":"tampered"}', encoding="utf-8")
    with pytest.raises(QUALIFICATION.QualificationError, match="digest mismatch"):
        QUALIFICATION.load_evidence(path)


@pytest.mark.skipif(
    os.name == "nt", reason="symlink creation is not generally available"
)
def test_symlinked_receipt_is_rejected(tmp_path: Path):
    path = write(tmp_path / "symlink.json", evidence("macos-arm64"))
    document = json.loads(path.read_text(encoding="utf-8"))
    receipt_path = path.parent / document["receipt_manifest"]["scheduler"]["path"]
    original = receipt_path.with_suffix(".original")
    receipt_path.rename(original)
    receipt_path.symlink_to(original.name)
    with pytest.raises(QUALIFICATION.QualificationError, match="symlink"):
        QUALIFICATION.load_evidence(path)


def test_provider_summary_cannot_override_a_failed_login_receipt(tmp_path: Path):
    value = evidence("macos-arm64")
    login = value["_receipt_documents"]["codex-login"]
    login["login_mode"] = "failed"
    login["exit_code"] = 1
    path = write(tmp_path / "forged-provider-summary.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="summary mismatches"):
        QUALIFICATION.load_evidence(path)


def test_kiro_subscription_login_requires_api_key_removal(tmp_path: Path):
    value = evidence("macos-arm64")
    login = value["_receipt_documents"]["kiro-login"]
    login["removed_environment"].remove("KIRO_API_KEY")
    path = write(tmp_path / "kiro-api-key-not-removed.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="summary mismatches"):
        QUALIFICATION.load_evidence(path)


def test_claude_subscription_login_requires_alternate_credentials_removed(
    tmp_path: Path,
):
    value = evidence("macos-arm64")
    login = value["_receipt_documents"]["claude-login"]
    login["removed_environment"].remove("ANTHROPIC_FOUNDRY_API_KEY")
    path = write(tmp_path / "claude-foundry-key-not-removed.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="summary mismatches"):
        QUALIFICATION.load_evidence(path)


def test_provider_requires_escape_negative_resource_confinement_for_every_selection(
    tmp_path: Path,
):
    value = evidence("macos-arm64")
    value["_receipt_documents"]["codex-confinement-b"]["escape_probe"]["outcome"] = (
        "allowed"
    )
    path = write(tmp_path / "unconfined.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="resource confinement"):
        QUALIFICATION.load_evidence(path)

    value = evidence("macos-arm64")
    value["providers"][0]["evidence_refs"].remove("codex-confinement-b")
    del value["_receipt_documents"]["codex-confinement-b"]
    path = write(tmp_path / "missing-selection-confinement.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="assertion summary"):
        QUALIFICATION.load_evidence(path)


def test_provider_native_delegation_cannot_bypass_roster_caps(tmp_path: Path):
    value = evidence("macos-arm64")
    process = value["_receipt_documents"]["codex-process"]
    process["delegated_process_ids"] = [process["owned_process_ids"][1]]
    path = write(tmp_path / "unaccounted-delegation.json", value)

    with pytest.raises(QUALIFICATION.QualificationError, match="summary mismatches"):
        QUALIFICATION.load_evidence(path)


@pytest.mark.parametrize("roster_size", [8, 17])
def test_roster_is_bounded_to_the_application_contract(
    tmp_path: Path, roster_size: int
):
    value = evidence("macos-arm64")
    value["roster_size"] = roster_size
    path = write(tmp_path / f"roster-{roster_size}.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="roster_size"):
        QUALIFICATION.load_evidence(path)


def test_rejects_unadvertised_or_mismatched_native_target(tmp_path: Path):
    value = evidence("windows-x64")
    value["platform"]["arch"] = "arm64"
    path = write(tmp_path / "wrong-arch.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="does not match"):
        QUALIFICATION.load_evidence(path)

    value = evidence("windows-x64")
    value["package"]["target"] = "windows-arm64"
    path = write(tmp_path / "unadvertised.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="not an advertised"):
        QUALIFICATION.load_evidence(path)


def test_rejects_forged_machine_and_provider_concurrency_summaries(tmp_path: Path):
    value = evidence("macos-arm64")
    value["execution"]["maximum_simultaneous_invocations"] = 5
    path = write(tmp_path / "machine-limit.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="concurrency summary"):
        QUALIFICATION.load_evidence(path)

    value = evidence("macos-arm64")
    provider_summary(value, "codex")["effective_cap"] = 3
    path = write(tmp_path / "provider-cap.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="effective cap"):
        QUALIFICATION.load_evidence(path)


def test_pass_requires_receipt_derived_priority_budget_and_timer(tmp_path: Path):
    value = evidence("macos-arm64")
    value["execution"]["budget_policy"]["pause_observed"] = False
    path = write(tmp_path / "no-budget-pause.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="budget summary"):
        QUALIFICATION.load_evidence(path)


def test_scenario_summary_cannot_change_exact_head_or_action(tmp_path: Path):
    path = write(tmp_path / "wrong-head.json", evidence("macos-arm64"))
    value = json.loads(path.read_text(encoding="utf-8"))
    value["scenarios"]["sourced_report"]["head"]["room_seq"] = 43
    path.write_bytes(canonical(value))
    with pytest.raises(QUALIFICATION.QualificationError, match="head summary"):
        QUALIFICATION.load_evidence(path)

    path = write(tmp_path / "wrong-action.json", evidence("macos-arm64"))
    value = json.loads(path.read_text(encoding="utf-8"))
    value["scenarios"]["sourced_report"]["action_id"] = "unrelated-action"
    path.write_bytes(canonical(value))
    with pytest.raises(QUALIFICATION.QualificationError, match="action_id summary"):
        QUALIFICATION.load_evidence(path)


def test_report_and_code_receipts_require_independent_exact_result_lineage(
    tmp_path: Path,
):
    value = evidence("macos-arm64")
    summary_lineage = value["scenarios"]["reviewed_code_change"]["result_lineage"]
    receipt_lineage = value["_receipt_documents"]["scenario-reviewed_code_change"][
        "result_lineage"
    ]
    summary_lineage["reviewer_member_key"] = summary_lineage["producer_member_key"]
    receipt_lineage["reviewer_member_key"] = receipt_lineage["producer_member_key"]
    path = write(tmp_path / "self-review.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="independent review"):
        QUALIFICATION.load_evidence(path)

    value = evidence("macos-arm64")
    value["scenarios"]["sourced_report"]["result_lineage"]["artifact_sha256"] = "0" * 64
    value["_receipt_documents"]["scenario-sourced_report"]["result_lineage"][
        "artifact_sha256"
    ] = "0" * 64
    path = write(tmp_path / "wrong-artifact.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="does not bind"):
        QUALIFICATION.load_evidence(path)


def test_rejects_missing_explicit_provider_effort_in_receipt(tmp_path: Path):
    value = evidence("macos-arm64")
    value["_receipt_documents"]["codex-invocation-0"]["requested"]["effort"] = None
    path = write(tmp_path / "missing-effort.json", value)
    with pytest.raises(QUALIFICATION.QualificationError, match="bounded non-empty"):
        QUALIFICATION.load_evidence(path)


def test_rejects_revision_mismatch(tmp_path: Path):
    path = write(tmp_path / "macos.json", evidence("macos-arm64"))
    with pytest.raises(QUALIFICATION.QualificationError, match="revision mismatch"):
        QUALIFICATION.assemble([path], "0" * 40)


def test_cli_retains_failure_report_and_returns_nonzero(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    evidence_path = write(tmp_path / "macos.json", evidence("macos-arm64"))
    output = tmp_path / "report.json"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "agent-swarm-qualification.py",
            "--evidence",
            str(evidence_path),
            "--expected-revision",
            REVISION,
            "--output",
            str(output),
        ],
    )
    assert QUALIFICATION.main() == 1
    report = json.loads(output.read_text(encoding="utf-8"))
    assert report["status"] == "fail"
    assert report["missing_targets"] == ["macos-x86_64", "windows-x64"]


def test_cli_emits_self_contained_provider_qualification_bundle(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    paths = complete_paths(tmp_path)
    output = tmp_path / "report.json"
    qualification_directory = tmp_path / "qualifications"
    arguments = ["agent-swarm-qualification.py"]
    for path in paths:
        arguments.extend(["--evidence", str(path)])
    arguments.extend(
        [
            "--expected-revision",
            REVISION,
            "--output",
            str(output),
            "--provider-qualification-dir",
            str(qualification_directory),
        ]
    )
    monkeypatch.setattr(sys, "argv", arguments)

    assert QUALIFICATION.main() == 0
    record = json.loads(
        (qualification_directory / "macos-arm64-codex.json").read_text(encoding="utf-8")
    )
    evidence_path = qualification_directory / record["evidence_path"]
    assert (
        hashlib.sha256(evidence_path.read_bytes()).hexdigest()
        == record["evidence_sha256"]
    )
    bundled = json.loads(evidence_path.read_text(encoding="utf-8"))
    receipt = bundled["receipt_manifest"]["codex-login"]
    receipt_path = evidence_path.parent / receipt["path"]
    assert hashlib.sha256(receipt_path.read_bytes()).hexdigest() == receipt["sha256"]
