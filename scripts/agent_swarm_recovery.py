"""Instructed baseline-first live recovery through production Swarm autonomy.

The evaluator supplies a frozen, defective starter and requires its exact
reproduction as Candidate v1. Every subsequent choice remains with the planner.
"""

from __future__ import annotations

import hashlib
import json
import os
import signal
import subprocess
import tempfile
from pathlib import Path

from agent_swarm_autonomous_delivery import (
    AutonomousDeliveryTrial,
    authorized_checker_command,
    autonomous_delivery_policy,
)
from agent_swarm_challenge import file_sha256, require
from agent_swarm_live import CHECKER, CRITERION, GOAL
from blake3 import blake3

REPO = Path(__file__).resolve().parents[1]
DELIVERED_SOURCE = REPO / "docs/evidence/agent-swarm-autonomous-delivery/csv_tool.py"
BASELINE_FIXTURE = REPO / "scripts/fixtures/agent_swarm_recovery_baseline.py"
TEST_DESIGN = REPO / "docs/evidence/agent-swarm-autonomous-recovery/TEST-DESIGN.md"
PACK_BUNDLE = REPO / "packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack"
DELIVERED_SHA256 = "9e1400dc99e4fded1724961a11c04cdde9328864bdb50babae84bde739562d78"
BASELINE_SHA256 = "51963d5fd5af026ef5c70a0d0889d04c9e69a0f41a8246fda4db560e8f7f206a"
REMOVED_CALL = b"    _validate_quoting(text)\n"


def frozen_baseline() -> bytes:
    """Reject fixture drift, including changes beyond the one declared deletion."""
    delivered = DELIVERED_SOURCE.read_bytes()
    baseline = BASELINE_FIXTURE.read_bytes()
    require(hashlib.sha256(delivered).hexdigest() == DELIVERED_SHA256,
            "previous delivered source changed")
    require(delivered.count(REMOVED_CALL) == 1,
            "previous source no longer has one quote-validation call")
    require(baseline == delivered.replace(REMOVED_CALL, b"", 1),
            "recovery fixture differs beyond its declared one-line deletion")
    require(hashlib.sha256(baseline).hexdigest() == BASELINE_SHA256,
            "recovery fixture digest changed")
    return baseline


def recovery_goal(baseline: bytes) -> str:
    goal = (
        "Repair the supplied csv_tool.py regression. This is an instructed "
        "baseline-first reproduction trial. Before changing source, use ordinary "
        "Agent Swarm work and integration to submit Candidate version 1 with "
        "the exact UTF-8 starter bytes in the numbered source constraints. "
        "Decode each part's JSON string and concatenate in order without "
        "separators. The source SHA256 is "
        + hashlib.sha256(baseline).hexdigest()
        + ". Then select the configured check "
        "and observe its actual result. Do not claim it passed or deliver "
        "that defective baseline. After the failed check, diagnose the "
        "evidence, author a revised Candidate, run fresh checks, obtain an "
        "independent review, and deliver only an accepted repair. The starter "
        "is also the registered resource version 1. Preserve its bytes exactly "
        "for the first Candidate; the evaluator will verify identity. "
        + " ".join(GOAL.splitlines())
    )
    require(len(goal.encode("utf-8")) <= 4096,
            "recovery goal exceeds managed backend bound")
    return goal


def recovery_source_constraints(baseline: bytes) -> list[str]:
    """Losslessly carry the same frozen source through bounded Room constraints."""
    lines = baseline.decode("utf-8").splitlines(keepends=True)
    chunks: list[str] = []
    current = ""
    for line in lines:
        if current and len((current + line).encode("utf-8")) > 1500:
            chunks.append(current)
            current = ""
        current += line
    if current:
        chunks.append(current)
    require("".join(chunks).encode("utf-8") == baseline,
            "source constraint chunks changed baseline bytes")
    constraints = [
        f"Source part {index}/{len(chunks)}, JSON string: "
        + json.dumps(chunk, ensure_ascii=True)
        for index, chunk in enumerate(chunks, 1)
    ]
    require(all(not any(ord(char) < 32 or ord(char) == 127 for char in item)
                for item in constraints),
            "starter source constraint contains setup-forbidden control text")
    require(all(len(item.encode("utf-8")) <= 2048 for item in constraints),
            "starter source constraint exceeds Room bound")
    return constraints


def baseline_checker_preflight(baseline: bytes) -> dict[str, object]:
    """Run the unchanged authorized checker against the declared starter."""
    program, arguments = authorized_checker_command()
    with tempfile.TemporaryDirectory(prefix="swarm-recovery-preflight-") as directory:
        Path(directory, "csv_tool.py").write_bytes(baseline)
        process = subprocess.Popen(
            [program, *arguments], cwd=directory, env={}, start_new_session=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=20)
        except subprocess.TimeoutExpired as error:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate()
            raise RuntimeError("recovery baseline checker preflight timed out") from error
    try:
        output = json.loads(stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("recovery baseline checker did not return JSON") from error
    require(process.returncode == 1 and stderr == ""
            and output == {"cases": 26, "passed": 25,
                           "failed": ["quote_in_unquoted_field"]},
            "frozen baseline does not exhibit the predeclared real check failure")
    return {
        "exit_code": process.returncode,
        "output": output,
        "stdout": stdout,
        "stderr": stderr,
        "checker_sha256": hashlib.sha256(CHECKER.encode()).hexdigest(),
        "checker": {"program": program, "arguments": arguments, "environment": {}},
    }


def recovery_declaration(
    baseline: bytes, preflight: dict, process_guard: Path,
) -> dict[str, object]:
    """One frozen contract used before the trial and at autonomy setup."""
    require(hashlib.sha256(baseline).hexdigest() == BASELINE_SHA256,
            "declaration baseline identity differs from frozen fixture")
    criteria_bytes = json.dumps(
        [CRITERION], ensure_ascii=False, separators=(",", ":")
    ).encode()
    program = Path(preflight["checker"]["program"])
    return {
        "schema": "worldstream/agent-swarm-recovery-declaration@1",
        "baseline_first_instructed": True,
        "baseline_source_sha256": "sha256:" + BASELINE_SHA256,
        "baseline_source_byte_length": len(baseline),
        "goal_sha256": "sha256:" + hashlib.sha256(
            recovery_goal(baseline).encode()
        ).hexdigest(),
        "source_constraint_digests": [
            "sha256:" + hashlib.sha256(item.encode()).hexdigest()
            for item in recovery_source_constraints(baseline)
        ],
        "acceptance_criteria": [CRITERION],
        "criteria_bytes": criteria_bytes.decode(),
        "criteria_digest": "blake3:" + blake3(criteria_bytes).hexdigest(),
        "criteria_revision": 1,
        "required_check_ids": ["criterion-1"],
        "fixed_check_count": 26,
        "checker": preflight["checker"],
        "checker_executable_sha256": file_sha256(program),
        "process_guard_sha256": file_sha256(process_guard),
        "fixed_checker_sha256": preflight["checker_sha256"],
        "input_digests": {
            "baseline_fixture": file_sha256(BASELINE_FIXTURE),
            "previous_source_evaluator_only": file_sha256(DELIVERED_SOURCE),
            "test_design": file_sha256(TEST_DESIGN),
            "pack_bundle": file_sha256(PACK_BUNDLE),
        },
        "policy": autonomous_delivery_policy(process_guard),
        "preflight": preflight,
    }


def export_exact_evidence(root: Path, autonomy_root: Path, versions: list[dict]) -> dict:
    """Retain independently verifiable bytes, not just summarized check labels."""
    export = root / "recovery-evidence"
    export.mkdir(exist_ok=False)
    manifest = {"candidates": [], "checks": [], "criteria": [], "provenance": []}

    def put(relative: str, data: bytes, expected_digest: str | None = None,
            expected_length: int | None = None) -> dict:
        digest = "blake3:" + blake3(data).hexdigest()
        require(expected_digest is None or digest == expected_digest,
                f"exported evidence digest mismatch: {relative}")
        require(expected_length is None or len(data) == expected_length,
                f"exported evidence length mismatch: {relative}")
        path = export / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return {"path": relative, "byte_length": len(data), "blake3": digest,
                "sha256": "sha256:" + hashlib.sha256(data).hexdigest()}

    for row in versions:
        version = row["candidate"]["version"]
        candidate = row["candidate"]["artifact"]
        source = Path(candidate["local_path"]).read_bytes()
        manifest["candidates"].append({"version": version, **put(
            f"candidates/version-{version}.py", source, candidate["digest"]
        )})
        object_root = autonomy_root / f"coordinator-service/delivery-v{version}/objects"
        for check in row["checks"]:
            room_check = check["room_check"]
            check_id = room_check["check_id"]
            evidence_ref = room_check["evidence_refs"][0]["artifact"]
            evidence_bytes = Path(evidence_ref["local_path"]).read_bytes()
            doc = json.loads(evidence_bytes)
            prefix = f"checks/version-{version}-{check_id}"
            record = {"version": version, "check_id": check_id,
                      "room_check_revision": room_check["revision"],
                      "evidence": put(prefix + ".json", evidence_bytes,
                                      evidence_ref["digest"])}
            for name in ("stdout", "stderr"):
                ref = doc[name]
                data = (object_root / ref["digest"].split(":", 1)[1]).read_bytes()
                record[name] = put(prefix + f".{name}", data,
                                   ref["digest"], ref["byte_length"])
            manifest["checks"].append(record)
            criteria_ref = doc["binding"]["criteria"]
            criteria_data = (
                object_root / criteria_ref["digest"].split(":", 1)[1]
            ).read_bytes()
            if not any(item["version"] == version for item in manifest["criteria"]):
                manifest["criteria"].append({"version": version, **put(
                    f"criteria/version-{version}.json", criteria_data,
                    criteria_ref["digest"], criteria_ref["byte_length"]
                )})

    journal = json.loads(
        (autonomy_root / "coordinator/coordinator.json").read_bytes()
    )
    safe_intents = {}
    for invocation_id, intent in journal["state"]["intents"].items():
        evidence = intent.get("evidence") or {}
        safe_intents[invocation_id] = {
            key: intent.get(key) for key in (
                "plan", "member_id", "proposal", "action", "state", "submission"
            )
        }
        safe_intents[invocation_id]["evidence"] = {
            key: evidence.get(key) for key in (
                "stdout", "resolution", "success", "exit_code", "output",
                "configuration",
            )
        }
        ref = evidence.get("stdout")
        if ref:
            digest = ref["digest"]
            data = (autonomy_root / "objects" / digest.split(":", 1)[1]).read_bytes()
            manifest["provenance"].append({
                "kind": "native_stdout", "invocation_id": invocation_id,
                **put(f"provenance/{invocation_id}.stdout", data,
                      digest, ref["byte_length"]),
            })
    provenance = json.dumps(
        {"schema": "worldstream/agent-swarm-recovery-provenance@1",
         "intents": safe_intents}, indent=2, sort_keys=True
    ).encode()
    manifest["provenance"].append({"kind": "selected_intents", **put(
        "provenance/selected-intents.json", provenance
    )})
    service = json.loads(
        (autonomy_root / "coordinator-service/service.json").read_bytes()
    )
    operations = service["state"]["autonomy"]["delivery"]["operations"]
    delivery_actions = json.dumps({
        "schema": "worldstream/agent-swarm-recovery-delivery-actions@1",
        "operations": {
            operation_id: {
                key: operation.get(key) for key in (
                    "target", "status", "actions", "writeback"
                )
            }
            for operation_id, operation in operations.items()
        },
    }, indent=2, sort_keys=True).encode()
    manifest["provenance"].append({"kind": "delivery_actions", **put(
        "provenance/delivery-actions.json", delivery_actions
    )})
    (export / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return {"root": str(export), "manifest": manifest}


class RecoveryTrial(AutonomousDeliveryTrial):
    def __init__(self, *args, **kwargs):
        self.baseline = frozen_baseline()
        self.baseline_preflight = baseline_checker_preflight(self.baseline)
        super().__init__(*args, **kwargs)

    def verify_setup_and_declare(self):
        require((self.work / "csv_tool.py").read_bytes() == self.baseline,
                "registered starter bytes differ from frozen baseline")
        resources = self.observe()["activity"]["resources"]
        require(any(
            resource["resource_id"] == "csv-target"
            and resource["version"] == 1
            and resource["digest"] == file_sha256(self.work / "csv_tool.py")
            for resource in resources
        ), "registered resource version 1 differs from frozen starter")
        self.persist("baseline-preflight", self.baseline_preflight)
        self.declaration = self.persist(
            "recovery-declaration",
            recovery_declaration(self.baseline, self.baseline_preflight, self.guard),
        )

    def initial_source(self):
        return self.baseline.decode("utf-8")

    def goal_text(self):
        return recovery_goal(self.baseline)

    def additional_constraints(self):
        return [
            "The supplied starter source is deliberately defective. First submit its exact unchanged bytes as Candidate version 1 and run the configured real checker; then choose the repair. Do not introduce another defect or weaken the fixed acceptance criterion.",
            *recovery_source_constraints(self.baseline),
        ]

    def run(self):
        self.verify_setup_and_declare()
        report = super().run()
        versions = report["candidate_versions"]
        require(len(versions) >= 2, "no model-authored repair Candidate followed baseline")
        first = versions[0]
        candidate = first["candidate"]
        require(candidate["version"] == 1, "first Candidate is not version 1")
        source = Path(candidate["artifact"]["local_path"]).read_bytes()
        require(source == self.baseline, "Candidate version 1 did not reproduce starter bytes")
        require(candidate["artifact"]["digest"] == "blake3:" + blake3(source).hexdigest(),
                "Candidate version 1 artifact digest mismatched")
        require(len(first["checks"]) == 1,
                "Candidate version 1 lacks its exact production Check")
        first_check = first["checks"][0]
        require(first_check["room_check"]["status"] == "failed",
                "Candidate version 1 was not recorded as a real failed Check")
        require(json.loads(first_check["stdout"]) == self.baseline_preflight["output"],
                "production failed-check output differs from frozen preflight")
        repaired = versions[-1]
        repaired_source = Path(repaired["candidate"]["artifact"]["local_path"]).read_bytes()
        require(repaired_source != self.baseline, "final Candidate did not revise the source")
        require(repaired["checks"] and all(
            check["room_check"]["status"] == "passed" for check in repaired["checks"]
        ), "revised Candidate did not pass fresh production checks")
        require(repaired["reviews"] and all(
            review["verdict"] == "passed" for review in repaired["reviews"]
        ), "revised Candidate lacks passing independent review")
        contributions = self.observe()["activity"]["contributions"]
        authors = {repaired["candidate"]["author_member_id"]}
        for ref in repaired["candidate"]["contribution_refs"]:
            matching = [item for item in contributions if
                        item["contribution_id"] == ref["contribution_id"]
                        and item["version"] == ref["version"]]
            require(len(matching) == 1, "revised Candidate source Contribution missing")
            authors.add(matching[0]["author_member_id"])
        require(any(review["reviewer_member_id"] not in authors
                    for review in repaired["reviews"]),
                "no independent reviewer assessed repaired Candidate")
        require(len(report["accepted_results"]) == 1 and
                report["accepted_results"][0]["candidate"] == {
                    "candidate_id": repaired["candidate"]["candidate_id"],
                    "version": repaired["candidate"]["version"],
                }, "revised Candidate was not accepted")
        delivered = (self.work / "csv_tool.py").read_bytes()
        require(delivered == repaired_source,
                "writeback target bytes differ from repaired Candidate")
        require(report["final_source_sha256"] ==
                "sha256:" + hashlib.sha256(repaired_source).hexdigest(),
                "writeback target digest differs from repaired Candidate")
        writebacks = [operation["writeback"] for operation in
                      report["planner_selected_delivery_operations"].values()
                      if operation["writeback"] is not None]
        require(len(writebacks) == 1 and len(writebacks[0]["targets"]) == 1,
                "no unique applied writeback target")
        observed = writebacks[0]["targets"][0]["outcome"]["observed"]
        require(observed == {
            "byte_length": len(repaired_source),
            "digest": repaired["candidate"]["artifact"]["digest"],
        }, "writeback observed digest differs from repaired Candidate")
        exported = export_exact_evidence(self.root, self.autonomy_root, versions)
        report.update({
            "schema": "worldstream/agent-swarm-live-recovery-evaluation@1",
            "baseline_first_instructed": True,
            "evaluator_supplied_initial_source": True,
            "baseline_source_sha256": "sha256:" + BASELINE_SHA256,
            "baseline_fixture": str(BASELINE_FIXTURE),
            "pre_run_declaration": str(self.declaration),
            "baseline_checker": self.baseline_preflight,
            "exported_evidence": exported,
            "limits": [
                "The baseline-first reproduction was required in the goal; this is not spontaneous defect discovery.",
                "The evaluator supplied the defective starter source before launch; subsequent Candidates and decisions were model-authored.",
                "One bounded single-file recovery; no general reliability rate or cross-run learning is claimed.",
            ],
        })
        return report
