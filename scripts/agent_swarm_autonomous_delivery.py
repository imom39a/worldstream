"""Live evaluation with no driver-authored delivery plans or post-setup Actions."""

from __future__ import annotations

import hashlib
import json
import sys
import time
from pathlib import Path

from agent_swarm_challenge import file_sha256, require
from agent_swarm_live import CHECKER, LiveTrial, verified_check_text
from blake3 import blake3


def authorized_checker_command(script=CHECKER):
    """Exact trusted argv builds Seatbelt around the actual isolated check cwd."""
    wrapper = """
import json, os, sys
from pathlib import Path
root = Path.cwd().resolve(strict=True)
python = Path(sys.executable).resolve(strict=True)
runtime = python.parents[1]
ancestors = sorted(set(root.parents) | set(runtime.parents) | {Path('/usr')})
readable = [root, runtime, Path('/System'), Path('/usr/lib'), Path('/private/etc')]
filters = ['(subpath ' + json.dumps(str(p)) + ')' for p in readable]
filters += ['(literal ' + json.dumps(str(p)) + ')' for p in ancestors]
filters += ['(literal "/dev/null")', '(literal "/dev/urandom")']
profile = '(version 1) (deny default) (allow process-exec process-fork file-map-executable) (allow signal (target self)) (allow sysctl-read mach-lookup file-read-metadata) (allow file-read-data ' + ' '.join(filters) + ')'
os.execv('/usr/bin/sandbox-exec', ['/usr/bin/sandbox-exec', '-p', profile, str(python), '-B', '-c', sys.argv[1]])
"""
    require(sys.platform == "darwin", "this live evaluation requires macOS Seatbelt")
    return str(Path(sys.executable).resolve(strict=True)), [
        "-B",
        "-c",
        wrapper,
        script,
    ]


def autonomous_delivery_policy(
    process_guard, *, target="csv_tool.py", resource_id="csv-target", checker=CHECKER
):
    program, arguments = authorized_checker_command(checker)
    return {
        "max_work_items": 4,
        "invocation_limit": 40,
        "resource_policy": "read_only",
        "allowed_tools": [],
        "delivery": {
            "target": target,
            "resource_id": resource_id,
            "expected_resource_version": 1,
            "maximum_candidate_versions": 3,
            "process_guard": str(process_guard.resolve(strict=True)),
            "checks": [
                {
                    "check_id": "criterion-1",
                    "program": program,
                    "arguments": arguments,
                    "timeout_seconds": 20,
                }
            ],
        },
    }


class AutonomousDeliveryTrial(LiveTrial):
    expected_terminal_phase = "completed"

    def checker_text(self):
        return CHECKER

    def require_parallel(self):
        return True

    def continuation_constraint(self):
        return (
            "The production planner owns the entire continuation. After useful Contributions exist, "
            "propose and claim integration work, submit a complete Candidate, select the configured "
            "check operations, request independent review, revise and resolve findings when needed, "
            "then select delivery. No evaluation driver will stage these steps. Wait does not complete "
            "the goal. Reserve judge for independent Candidate review and choose other agents for authorship."
        )

    def planning_policy(self):
        return autonomous_delivery_policy(
            self.guard,
            target=self.target_name(),
            resource_id=self.resource_id(),
            checker=self.checker_text(),
        )

    def run(self):
        service = self.ordinary_work()
        observation = self.observe()
        self.persist("final-observation", observation)
        activity = observation["activity"]
        require(activity["phase"] == "completed", "Room did not accept completion")
        require(self.invocations == 0, "evaluation driver staged a delivery plan")
        require(
            self.direct_actions == 2, "evaluation driver submitted a post-setup Action"
        )
        contributions = activity["contributions"]
        if self.require_parallel():
            require(
                len({c["author_member_id"] for c in contributions}) >= 2,
                "independent Contributions missing",
            )
        maximum = max(
            (len(s["native_running_without_completion"]) for s in self.samples),
            default=0,
        )
        if self.require_parallel():
            require(maximum >= 2, "parallel native work was not observed")
        state = json.loads(
            (self.autonomy_root / "coordinator-service/service.json").read_text()
        )["state"]["autonomy"]
        delivery = state["delivery"]
        self.persist("autonomous-delivery-journal", delivery)
        self.persist("autonomous-final-service", service)
        versions = []
        for candidate in activity["candidates"]:
            version = candidate["version"]
            checks = []
            for check in activity["checks"]:
                if check["candidate"] != {
                    "candidate_id": candidate["candidate_id"],
                    "version": version,
                }:
                    continue
                evidence_ref = check["evidence_refs"][0]["artifact"]
                data = Path(evidence_ref["local_path"]).read_bytes()
                require(
                    "blake3:" + blake3(data).hexdigest() == evidence_ref["digest"],
                    "Room check evidence digest mismatch",
                )
                evidence = json.loads(data)
                root = self.autonomy_root / f"coordinator-service/delivery-v{version}"
                checks.append(
                    {
                        "room_check": check,
                        "stdout": verified_check_text(root, evidence["stdout"]),
                        "stderr": verified_check_text(root, evidence["stderr"]),
                    }
                )
            versions.append(
                {
                    "candidate": candidate,
                    "checks": checks,
                    "reviews": [
                        r
                        for r in activity["reviews"]
                        if r["candidate"]
                        == {
                            "candidate_id": candidate["candidate_id"],
                            "version": version,
                        }
                    ],
                }
            )
        completed = [
            op
            for op in delivery["operations"].values()
            if op["status"] == "completed" and op["writeback"] is not None
        ]
        require(
            len(completed) == 1
            and completed[0]["writeback"]["disposition"] == "applied",
            "exact applied writeback missing",
        )
        self.persist("writeback-receipt", completed[0]["writeback"])
        return {
            "schema": "worldstream/agent-swarm-autonomous-delivery-evaluation@1",
            "status": "passed",
            "model": self.native.args.model,
            "effort": self.native.args.effort,
            "swarm_id": self.swarm_id,
            "room_id": self.room_id,
            "room_seq": observation["room_seq"],
            "authoritative_state_hash": observation["authoritative_state_hash"],
            "elapsed_seconds": round(time.monotonic() - self.started, 3),
            "adaptive_service": service,
            "max_observed_native_overlap": maximum,
            "native_samples": self.samples,
            "delivery_plans_staged_by_evaluation_driver": self.invocations,
            "evaluation_driver_actions": self.direct_actions,
            "evaluation_driver_post_setup_actions": 0,
            "candidate_versions": versions,
            "accepted_results": activity["results"],
            "planner_selected_delivery_operations": delivery["operations"],
            "final_source_sha256": file_sha256(self.work / self.target_name()),
            "delivered_path": str(self.work / self.target_name()),
            "fixed_checker_sha256": hashlib.sha256(
                self.checker_text().encode()
            ).hexdigest(),
            "fixed_check_count": 26,
            "limits": [
                "One bounded single-file goal under an explicit local delivery policy.",
                "No cross-run learning or portable release qualification is claimed.",
                "The evaluator supplies acceptance criteria and checker commands, not task decisions or source.",
            ],
        }
