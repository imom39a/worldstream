"""Exercise model-selected planning protocol using explicit controlled fixtures.

No worker plans or ordinary domain work Actions are supplied by this harness.
The controlled planner is state-sensitive test data, not evidence of reasoning
quality from a real model. The existing CSV challenge remains the review test.
"""

from __future__ import annotations

import json
import subprocess
import time
from collections import Counter
from pathlib import Path
from typing import Any

from agent_swarm_challenge import Trial, file_sha256, require


def summarize_recovery(intents: dict[str, Any]) -> dict[str, Any]:
    attempts: dict[str, list[dict[str, Any]]] = {}
    for invocation, intent in intents.items():
        plan = intent["plan"]
        target = plan.get("semantic_target", {})
        if target.get("kind") != "work_attempt":
            continue
        instruction = json.loads(plan["instruction"])
        submission = intent.get("submission", {})
        attempts.setdefault(target["work_id"], []).append({
            "invocation_id": invocation,
            "revised_instruction": "Retained process failure" in instruction.get("instructions", ""),
            "process_failed": submission == {"status": "not_submitted", "value": "process_failed"},
            "accepted": submission.get("status") == "received"
            and isinstance(submission.get("value"), dict)
            and submission["value"].get("status") == "accepted",
        })
    recovered = [records for records in attempts.values()
                 if any(item["process_failed"] and not item["revised_instruction"] for item in records)
                 and any(item["accepted"] and item["revised_instruction"] for item in records)]
    return {"recovered_work_count": len(recovered),
            "failed_invocation_count": sum(item["process_failed"] for records in attempts.values() for item in records),
            "attempts": [item for records in attempts.values() for item in records]}


class AutonomyTrial(Trial):
    def __init__(self, harness: Any, *, app: Path, process_guard: Path,
                 execution_control: Path, controlled_worker: Path, root: Path,
                 work: Path, local: list[str], execution_state: Path,
                 registered_swarm_ids: list[str], mode: str, invocation_limit: int,
                 fail_once: bool = False) -> None:
        self.h, self.app, self.guard = harness, app, process_guard
        self.control, self.worker = execution_control, controlled_worker
        self.root, self.work = root / f"autonomy-{mode}", work / f"autonomy-{mode}"
        self.root.mkdir()
        self.work.mkdir()
        self.local, self.execution_state = local, execution_state
        self.mode, self.cap = mode, 2
        self.action_ids = harness.ActionIds()
        self.direct_actions = self.invocations = self.human_reconciliations = 0
        self.samples: list[dict[str, Any]] = []
        self.started = time.monotonic()
        self.cycles = 0
        self.launched: set[str] = set()
        self.policy = {"max_work_items": 2, "invocation_limit": invocation_limit,
                       "resource_policy": "workspace_write", "allowed_tools": []}
        self.policy_path = self.persist("autonomy-policy", self.policy)
        goal = "Investigate requirements and verification for CSV parsing and formatting in two independent notes."
        if fail_once:
            goal += " CONTROLLED_FAIL_ONCE"
        created = self.app_command("create", "--request", str(self.persist("create", {
            "goal": goal, "constraints": ["Produce separate evidence notes; result acceptance requires checks and review."],
            "acceptance_criteria": [{"text": "Integrated notes have passed checks and independent review."}],
            "working_area": str(self.work), "progress_review_interval_seconds": 3600,
            "roster": [{"member_key": member, "label": member, "provider": "controlled",
                        "requested_model": "fixture-v1", "requested_effort": "medium",
                        "configuration_state": "fixture_unavailable"} for member in ("analyst", "verifier")],
        })), scoped=False)
        self.swarm_id, self.room_id = created["swarm_id"], created["room_id"]
        registered_swarm_ids.append(self.swarm_id)
        self.coordinator = [*local, "--swarm-id", self.swarm_id,
                            "--coordinator-state", str(self.root / "coordinator"),
                            "--controlled-path", str(self.worker)]
        self.act("confirm_initial_setup", {"setup_revision": self.observe()["activity"]["setup_revision"]})
        self.ctl("set-provider-cap", "controlled", "2")

    def cycle(self) -> dict[str, Any]:
        self.settle_operation = "autonomous-worker-run"
        try:
            receipt = self.command([str(self.app), "worker-run", *self.coordinator,
                                    "--autonomy-policy", str(self.policy_path), "--once"])
            self.cycles += 1
            service = receipt["service"]
            autonomy = service["autonomy"]
            require(autonomy["generated_invocation_count"] <= self.policy["invocation_limit"], "planning budget exceeded")
            require(autonomy["generated_work_count"] <= self.policy["max_work_items"], "work ceiling exceeded")
            self.launched.update(event["invocation_id"] for event in receipt.get("events", [])
                                 if event.get("event") == "launched")
            status = self.ctl("status", "--swarm-id", self.swarm_id)["value"]
            active = [entry["ticket"]["invocation_id"] for swarm in status["swarms"]
                      for entry in swarm["active"] if entry["resolution"] == "running"]
            require(len(active) <= self.cap, "provider cap exceeded")
            native = [invocation for invocation in sorted(set(active) & self.launched)
                      if self.native_is_running(invocation)]
            self.samples.append({"cycle": self.cycles, "native_running_without_completion": native})
            return service
        except (RuntimeError, subprocess.TimeoutExpired):
            self.capture_failure(sorted(self.launched), "autonomy_cycle_failed")
            raise

    def run(self, *, budget_only: bool, fail_once: bool) -> dict[str, Any]:
        self.ctl("resume", self.swarm_id)
        self.ctl("pause", self.swarm_id)
        paused = self.cycle()
        require(paused["autonomy"]["generated_invocation_count"] == 0
                and not self.observe()["activity"]["work_items"], "paused cycle generated work")
        self.ctl("resume", self.swarm_id)
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            service = self.cycle()
            activity = self.observe()["activity"]
            require(not activity["results"] and activity["phase"] == "open", "planner accepted an unchecked result")
            if budget_only and service["autonomy"]["phase"] == "budget_exhausted":
                break
            if not budget_only and service["autonomy"]["phase"] == "waiting":
                require(len(activity["work_items"]) == 2
                        and all(item["status"] == "completed" for item in activity["work_items"]),
                        "planner waited before the fixture's two contributions completed")
                break
            time.sleep(0.04)
        else:
            self.capture_failure(sorted(self.launched), "autonomy_deadline")
            raise RuntimeError("autonomous fixture did not reach its bounded stopping state")
        before = self.observe()
        identity = (before["room_seq"], service["autonomy"]["generated_invocation_count"],
                    sorted(item["invocation_id"] for item in service["invocations"]))
        for _ in range(3):
            service = self.cycle()
            after = self.observe()
            require(identity == (after["room_seq"], service["autonomy"]["generated_invocation_count"],
                                 sorted(item["invocation_id"] for item in service["invocations"])),
                    "reopening the coordinator repeated a retained planning decision")
        if budget_only:
            require(service["autonomy"]["generated_invocation_count"] == 1,
                    "one-invocation budget did not stop after the planning invocation")
        else:
            authors = {item["author_member_id"] for item in activity["contributions"]}
            require(len(authors) == 2 and len(activity["contributions"]) == 2,
                    "independent workers did not each contribute exactly once")
            require(all(not item["dependency_ids"] for item in activity["work_items"]),
                    "controlled fixture did not create independent tasks")
            for item in activity["contributions"]:
                artifact = Path(item["artifact"]["local_path"])
                require(artifact.is_relative_to(self.work) and item["work_id"] in artifact.read_text(),
                        "contribution artifact missing or outside authorized working area")
        retained = json.loads((self.root / "coordinator/coordinator/coordinator.json").read_text())["state"]["intents"]
        invocation_kinds = Counter(intent["plan"]["semantic_target"]["kind"] for intent in retained.values())
        recovery = summarize_recovery(retained)
        if fail_once:
            require(recovery["recovered_work_count"] == 1 and recovery["failed_invocation_count"] == 1,
                    "planner did not adapt instructions after exactly one retained failure")
        max_native = max(len(sample["native_running_without_completion"]) for sample in self.samples)
        if not budget_only and not fail_once:
            require(max_native == 2, "independent planned work did not overlap natively")
        self.ctl("stop", self.swarm_id)
        return {"mode": self.mode, "policy": self.policy, "autonomy": service["autonomy"],
                "room_id": self.room_id, "swarm_id": self.swarm_id,
                "room_seq": before["room_seq"], "authoritative_state_hash": before["authoritative_state_hash"],
                "work_item_count": len(activity["work_items"]), "contribution_count": len(activity["contributions"]),
                "contribution_artifacts": [{"work_id": item["work_id"],
                                            "artifact_id": item["artifact"]["artifact_id"],
                                            "digest": item["artifact"]["digest"]}
                                           for item in activity["contributions"]],
                "result_count": len(activity["results"]), "manually_staged_plan_count": 0,
                "human_work_action_count": 0, "scripted_setup_confirmation_count": self.direct_actions,
                "paused_cycle_generated_nothing": True, "restart_did_not_duplicate_work": True,
                "restart_scope": "The coordinator command process reopens each cycle; the execution daemon stays alive.",
                "max_observed_native_running": max_native, "concurrency_samples": self.samples,
                "recovery": recovery, "worker_run_process_count": self.cycles,
                "invocation_kinds": dict(sorted(invocation_kinds.items())),
                "planning_invocation_count": invocation_kinds["planning"],
                "worker_invocation_count": sum(invocation_kinds.values()) - invocation_kinds["planning"],
                "elapsed_seconds": round(time.monotonic() - self.started, 3)}


def run_autonomy(harness: Any, *, execution_daemon: Path, **arguments: Any) -> dict[str, Any]:
    identity_paths = {"application_sha256": arguments["app"], "daemon_sha256": execution_daemon,
                      "controlled_worker_sha256": arguments["controlled_worker"],
                      "process_guard_sha256": arguments["process_guard"],
                      "execution_control_sha256": arguments["execution_control"],
                      "autonomy_script_sha256": Path(__file__),
                      "challenge_helper_sha256": Path(__file__).with_name("agent_swarm_challenge.py"),
                      "managed_harness_sha256": Path(harness.__file__)}
    identities = {name: file_sha256(path) for name, path in identity_paths.items()}
    trials = [AutonomyTrial(harness, **arguments, mode=mode, invocation_limit=limit, fail_once=fail).run(
        budget_only=budget, fail_once=fail)
        for mode, limit, budget, fail in (("parallel", 16, False, False),
                                          ("recovery", 16, False, True),
                                          ("budget", 1, True, False))]
    require(all(file_sha256(path) == identities[name] for name, path in identity_paths.items()),
            "an executable or the autonomy harness changed during the verification run")
    local = arguments["local"]
    identities["pack_revision_digest"] = local[local.index("--pack-digest") + 1]
    return {"schema": "worldstream/agent-swarm-autonomy-protocol-check/v1", "status": "passed",
            "execution_kind": "controlled_state_sensitive_planner_fixture", "real_model_reasoning_tested": False,
            "manual_worker_plans": 0, "automatic_result_acceptance": False,
            "review_gate_tested_here": False,
            "review_gate_evidence": "The separate managed CSV challenge exercises exact checks and independent review.",
            "controlled_contribution_delay_ms": 1500, "controlled_adapter_work_attempt_delay_ms": 250,
            "identities": identities,
            "trials": trials}
