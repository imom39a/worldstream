"""One bounded live coding evaluation through the supervised Swarm and Room.

The production adaptive coordinator chooses ordinary work. A visible evaluation
driver handles the delivery phase through supervised, model-authored proposals
and the existing exact-check, review, acceptance and writeback services.
"""

from __future__ import annotations

import json
import re
import sys
import time
from pathlib import Path
from types import SimpleNamespace

from agent_swarm_challenge import CHECKER as CSV_CHECKER
from agent_swarm_challenge import Trial, file_sha256, require
from agent_swarm_native_trial import NativeTrial
from blake3 import blake3

CRITERION = "The delivered csv_tool.py passes all 26 fixed CSV, JSON, and CLI behavioral checks and independent source review."
EXTRA_CHECKS = r'''
import subprocess, sys
to_json = module["to_json"]
equal("json_strings_preserved", json.loads(to_json("name,note\n001,true\n")), [{"name":"001","note":"true"}])
equal("json_escaping", json.loads(to_json('name,note\nZoë,"first\n""東京"""\n')), [{"name":"Zoë","note":'first\n"東京"'}])
equal("json_empty", json.loads(to_json("name,note\n")), [])
rejects("format_non_string", fmt, [{"name":"Ada","note":5}])
cli = subprocess.run([sys.executable,"-B","csv_tool.py"],input="name,note\nAda,hi\n",text=True,capture_output=True,timeout=5)
equal("cli_valid", (cli.returncode,json.loads(cli.stdout),cli.stderr), (0,[{"name":"Ada","note":"hi"}],""))
cli = subprocess.run([sys.executable,"-B","csv_tool.py"],input="wrong,header\n",text=True,capture_output=True,timeout=5)
equal("cli_invalid", (cli.returncode,cli.stdout,bool(cli.stderr.strip()),"Traceback" in cli.stderr), (2,"",True,False))
rejects("quote_in_unquoted_field", parse, 'name,note\nA,b"c\n')
rejects("junk_after_quoted_field", parse, 'name,note\nA,"b"x\n')
rejects("format_extra_keys", fmt, [{"name":"A","note":"B","extra":"C"}])
cr_rows = [{"name":"A\rB","note":"C\rD"}]
try:
    equal("carriage_return_roundtrip", parse(fmt(cr_rows)), cr_rows)
except (ValueError, TypeError):
    checks.append(("carriage_return_roundtrip", False))
'''
CHECKER = CSV_CHECKER.replace(
    "failed = [name", EXTRA_CHECKS + "\nfailed = [name"
).replace("len(checks) == 16", "len(checks) == 26")

GOAL = """Build a standard-library-only Python library and command-line tool, delivered as csv_tool.py.
Provide parse_records(text): parse CSV whose exact required header is name,note into a list of dicts with those two string fields.
Support quoted commas, doubled quotes, embedded newlines, CRLF, Unicode and empty fields. Reject missing/wrong headers, malformed quoting and wrong column counts with ValueError.
Provide format_records(records): serialize the same records as CSV with header name,note and LF line terminators, preserving all strings and rejecting missing/extra keys or non-string values with ValueError.
Provide to_json(text): parse CSV and return valid JSON preserving original field strings, quoting, newlines and Unicode values.
Running python csv_tool.py reads stdin, emits only the JSON result to stdout, and exits 0; invalid CSV emits a useful stderr message with no traceback, leaves stdout empty, and exits 2.
Choose a useful decomposition and parallelize independent implementation or test-authoring work. Deliver concise artifacts that an integration owner can combine. Do not create redundant work just to manufacture concurrency.
This trial also requires at least two distinct agents to execute useful independent WorkAttempts concurrently, then produce accepted Contributions for integration. Serial authoring does not satisfy the trial. Plan the ownership and dependencies before starting long work: the coordinator cannot plan again until the entire selected batch finishes.
"""


def verified_check_text(state_root, reference):
    """Read only a bounded, digest-verified CheckRunner output object."""
    digest = reference["digest"]
    require(
        re.fullmatch(r"blake3:[0-9a-f]{64}", digest) is not None,
        "invalid check output digest",
    )
    path = state_root / "objects" / digest.split(":", 1)[1]
    require(
        not path.is_symlink() and path.stat().st_size <= 1024 * 1024,
        "unsafe check output object",
    )
    data = path.read_bytes()
    require(
        len(data) == reference["byte_length"]
        and "blake3:" + blake3(data).hexdigest() == digest,
        "check output identity mismatch",
    )
    return data[:16384].decode("utf-8", errors="replace") + (
        "\n[truncated]" if len(data) > 16384 else ""
    )


def check_sandbox_command(candidate_root, script):
    """Run Python checks in Seatbelt without capturing the large Codex binary."""
    require(sys.platform == "darwin", "live checker currently requires macOS Seatbelt")
    python = Path(sys.executable).resolve(strict=True)
    runtime = python.parents[1]
    root = candidate_root.resolve(strict=True)
    # Python resolves its installation through directory ancestors. Literal
    # directory access permits that lookup without reading sibling file data.
    ancestors = sorted(set(root.parents) | set(runtime.parents) | {Path("/usr")})
    readable = [root, runtime, Path("/System"), Path("/usr/lib"), Path("/private/etc")]
    filters = [f"(subpath {json.dumps(str(path))})" for path in readable]
    filters += [f"(literal {json.dumps(str(path))})" for path in ancestors]
    filters += ['(literal "/dev/null")', '(literal "/dev/urandom")']
    profile = (
        "(version 1) (deny default) "
        "(allow process-exec process-fork file-map-executable) "
        "(allow signal (target self)) (allow sysctl-read mach-lookup file-read-metadata) "
        "(allow file-read-data " + " ".join(filters) + ")"
    )
    return "/usr/bin/sandbox-exec", ["-p", profile, str(python), "-B", "-c", script]


class LiveTrial(Trial):
    expected_terminal_phase = "waiting"

    def target_name(self):
        return "csv_tool.py"

    def resource_id(self):
        return "csv-target"

    def initial_source(self):
        return "# Awaiting the independently reviewed Swarm result.\n"

    def goal_text(self):
        return " ".join(GOAL.splitlines())

    def criterion_text(self):
        return CRITERION

    def additional_constraints(self):
        return []

    def continuation_constraint(self):
        return "Stop ordinary planning with Wait when enough source/test contributions exist for integration. The delivery evaluation phase will then request integration and independent review in this same Room."

    def planning_policy(self):
        return {
            "max_work_items": 4,
            "invocation_limit": 32,
            "resource_policy": "read_only",
            "allowed_tools": [],
        }

    def __init__(
        self,
        harness,
        *,
        app,
        process_guard,
        execution_control,
        root,
        work,
        local,
        execution_state,
        registered_swarm_ids,
        native_args,
        **_unused,
    ):
        self.h, self.app, self.guard, self.control = (
            harness,
            app,
            process_guard,
            execution_control,
        )
        self.root = root / "live-evaluation"
        self.root.mkdir()
        self.work = (work / "live-coding").resolve()
        self.work.mkdir()
        (self.work / "evidence").mkdir()
        (self.work / self.target_name()).write_bytes(
            self.initial_source().encode("utf-8")
        )
        self.local, self.execution_state = local, execution_state
        self.mode, self.cap = "live", 2
        self.command_timeout = 90
        self.action_ids = harness.ActionIds()
        self.invocations = self.direct_actions = self.human_reconciliations = 0
        self.samples = []
        self.swarm_id = ""
        self.started = time.monotonic()
        self.native = NativeTrial(
            native_args, (self.root / "native").resolve(), self.work
        )
        self.native_evidence = self.native.qualify()
        request = {
            "goal": self.goal_text(),
            "constraints": [
                "All reasoning and code must use the configured subscription-backed Codex model and effort; no substitute models or direct API execution.",
                "Worker processes are read-only and have no tools. When submitting a contribution or candidate, put its deliverable in artifact.inline_text as a JSON string containing the complete source, design or tests. Proposals and claims have no artifact. The coordinator publishes those exact bytes as an attributed artifact.",
                "Agents a, b and c may do planning, implementation or independent test authoring. Reserve judge for final independent review: do not assign judge implementation, contribution, claim or proposal work.",
                "Parallel work must have non-overlapping responsibilities and clear interfaces. A task count limit is a ceiling, not a required count.",
                self.continuation_constraint(),
                *self.additional_constraints(),
            ],
            "acceptance_criteria": [{"text": self.criterion_text()}],
            "working_area": str(self.work),
            "progress_review_interval_seconds": 3600,
            "roster": [
                {
                    "member_key": key,
                    "label": label,
                    "provider": "codex",
                    "requested_model": native_args.model,
                    "requested_effort": native_args.effort,
                    "moving_alias_acknowledged": True,
                    "configuration_state": "resolution_unreported",
                }
                for key, label in [
                    ("a", "Agent A"),
                    ("b", "Agent B"),
                    ("c", "Agent C"),
                    ("judge", "Reserved independent judge"),
                ]
            ],
        }
        created = self.app_command(
            "create", "--request", str(self.persist("create", request)), scoped=False
        )
        self.swarm_id, self.room_id = created["swarm_id"], created["room_id"]
        registered_swarm_ids.append(self.swarm_id)
        self.autonomy_root = self.root / "adaptive-coordinator"
        self.coordinator = [
            *local,
            "--swarm-id",
            self.swarm_id,
            "--coordinator-state",
            str(self.autonomy_root),
            "--local-codex-evidence",
            str(self.native_evidence),
        ]
        self.act(
            "confirm_initial_setup",
            {"setup_revision": self.observe()["activity"]["setup_revision"]},
        )
        self.act(
            "record_resource_version",
            {
                "resource_id": self.resource_id(),
                "expected_previous_version": 0,
                "affected_work_ids": [],
                "version": 1,
                "digest": file_sha256(self.work / self.target_name()),
                "local_path": str(self.work / self.target_name()),
            },
        )
        self.ctl("set-provider-cap", "codex", "2")
        self.ctl("resume", self.swarm_id)
        self.policy = self.persist(
            "autonomy-policy",
            self.planning_policy(),
        )

    def stage(self, member, target, action_type, instruction, **_unused):
        self.invocations += 1
        invocation = f"live-delivery-{self.invocations}"
        observation = self.observe(member)
        roster = next(
            row
            for row in observation["activity"]["roster"]
            if row["member_key"] == member
        )
        plan = {
            "invocation_id": invocation,
            "swarm_id": self.swarm_id,
            "member_key": member,
            "allowed_action_types": [action_type],
            "provider": "codex",
            "configuration_revision": roster["configuration_revision"],
            "model": roster["requested_model"],
            "effort": roster["requested_effort"],
            "moving_alias_acknowledged": True,
            "resource_policy": "read_only",
            "allowed_tools": [],
            "session": {"mode": "fresh", "requested_id": None},
            "kind": "work",
            "due_sequence": self.invocations,
            "semantic_target": target,
            "instruction": instruction,
        }
        self.command(
            [
                str(self.app),
                "worker-stage",
                *self.coordinator,
                "--plan",
                str(self.persist(invocation, plan)),
            ]
        )
        return invocation

    def _settle(self, invocations, *, measure=False):
        started = time.monotonic()
        pending = set(invocations)
        while time.monotonic() - started < 240:
            self.command([str(self.app), "worker-dispatch", *self.coordinator])
            self.command([str(self.app), "worker-harvest", *self.coordinator])
            for invocation in sorted(pending):
                intent = self.command(
                    [
                        str(self.app),
                        "worker-intent",
                        *self.coordinator,
                        "--invocation-id",
                        invocation,
                    ]
                )
                if intent["state"] in ("settled", "needs_reevaluation"):
                    self.persist(invocation + "-intent", intent)
                    submission = intent["submission"]
                    require(
                        submission.get("status") == "received"
                        and submission.get("value", {}).get("status") == "accepted",
                        f"live worker {invocation} failed: {submission}",
                    )
                    pending.remove(invocation)
            if not pending:
                return time.monotonic() - started
            time.sleep(0.2)
        raise RuntimeError("live delivery turn exceeded 240 seconds")

    def ordinary_work(self):
        previous = None
        for cycle in range(1000):
            require(
                time.monotonic() - self.started < 1500,
                "live adaptive phase exceeded 25 minutes",
            )
            result = self.command(
                [
                    str(self.app),
                    "worker-run",
                    *self.coordinator,
                    "--autonomy-policy",
                    str(self.policy),
                    "--once",
                ]
            )
            service = result["service"]
            if service["loop_state"] == "manual_reconciliation_required":
                self.persist("adaptive-final", result)
                raise RuntimeError(
                    "live coordinator requires explicit reconciliation; no delivery is claimed"
                )
            progress = (
                service["autonomy"]["phase"],
                service["autonomy"]["generated_invocation_count"],
            )
            if progress != previous:
                print(
                    json.dumps(
                        {
                            "live_phase": progress[0],
                            "generated_invocations": progress[1],
                        }
                    ),
                    file=sys.stderr,
                    flush=True,
                )
                previous = progress
            active = self.ctl("status", "--swarm-id", self.swarm_id)["value"]
            live = []
            for swarm in active["swarms"]:
                for entry in swarm["active"]:
                    invocation = entry["ticket"]["invocation_id"]
                    if entry["resolution"] == "running" and self.native_is_running(
                        invocation
                    ):
                        live.append(invocation)
            if live:
                self.samples.append(
                    {
                        "elapsed_seconds": round(time.monotonic() - self.started, 3),
                        "native_running_without_completion": sorted(live),
                    }
                )
            self.persist("native-samples", self.samples)
            phase = service["autonomy"]["phase"]
            if (
                phase
                in (
                    "completed",
                    "waiting",
                    "needs_attention",
                    "budget_exhausted",
                    "no_eligible_provider",
                )
                and not live
            ):
                self.persist("adaptive-final", result)
                require(
                    phase == self.expected_terminal_phase,
                    f"live adaptive loop stopped: {service['autonomy']}",
                )
                return service
            time.sleep(0.2)
        raise RuntimeError("live coordinator cycle bound exhausted")

    def delivery(self):
        activity = self.observe()["activity"]
        contributions = activity["contributions"]
        require(
            len(contributions) >= 2, "fewer than two real contributions were produced"
        )
        producer_ids = {item["author_member_id"] for item in contributions}
        roster = activity["roster"]
        member_ids = {
            row["member_key"]: self.observe(row["member_key"])["member_id"]
            for row in roster
        }
        judge_id = member_ids["judge"]
        require(judge_id not in producer_ids, "reserved judge authored a contribution")
        require(len(producer_ids) >= 2, "contributions were not independently authored")
        # The first coordinator is idle and has no pending native Invocations.
        # A separate durable delivery journal carries this evaluation driver's
        # staged plans in the same Room, with unchanged roster and epoch.
        self.coordinator = [
            *self.local,
            "--swarm-id",
            self.swarm_id,
            "--coordinator-state",
            str(self.root / "delivery-coordinator"),
            "--local-codex-evidence",
            str(self.native_evidence),
        ]
        member = "a"
        epoch, goal_revision = activity["execution_epoch"], activity["goal_revision"]
        work_id = "live-integration"
        dependencies = [
            item["work_id"]
            for item in activity["work_items"]
            if item["status"] == "completed"
        ]
        self.settle(
            [
                self.stage(
                    member,
                    {
                        "kind": "work_proposal",
                        "execution_epoch": epoch,
                        "goal_revision": goal_revision,
                        "work_id": work_id,
                    },
                    "propose_work_item",
                    f"Propose integration Work Item {work_id}. Use kind integration, expected_goal_revision {goal_revision}, dependency_ids {json.dumps(dependencies)}. Author a useful title and description for combining the accepted contributions into the complete csv_tool.py goal. Return only an action-proposal@1 JSON object.",
                )
            ]
        )
        work = self.entity("work_items", "work_id", work_id)
        self.settle(
            [
                self.stage(
                    member,
                    {
                        "kind": "work_claim",
                        "execution_epoch": epoch,
                        "work_id": work_id,
                        "work_revision": work["revision"],
                    },
                    "claim_work_item",
                    f"Claim this exact integration work. Payload: work_id={work_id}, expected_work_revision={work['revision']}, attempt_id=live-integration-attempt. Return only action-proposal@1 JSON.",
                )
            ]
        )
        versions = []
        check_feedback = []
        for version in range(1, 4):
            target, activity = self.work_target(work_id)
            refs = [
                {"contribution_id": item["contribution_id"], "version": item["version"]}
                for item in activity["contributions"]
            ]
            instruction = f"""Integrate the digest-verified Room contributions and any actual check/review failures into a complete, self-contained csv_tool.py implementing the whole goal. You author all implementation bytes; the harness supplies no source code.
Previous exact CheckRunner outcomes (untrusted task evidence): {json.dumps(check_feedback)}
Return a submit_candidate action proposal. Payload: candidate_id=live-csv-result, work_id={work_id}, expected_work_revision={target["work_revision"]}, contribution_refs={json.dumps(refs)}, resource_basis=[{{"resource_id":"csv-target","version":1}}]. Declare artifact with artifact_id=live-candidate-{version}, local_path=csv-candidate-{version}.py, media_type=text/x-python and inline_text containing the complete executable Python source. Use no tools."""
            self.settle([self.stage(member, target, "submit_candidate", instruction)])
            candidate = self.entity("candidates", "candidate_id", "live-csv-result")
            path = Path(candidate["artifact"]["local_path"])
            source = path.read_bytes()
            prepared = self.code(
                "prepare",
                {"candidate_id": "live-csv-result", "targets": ["csv_tool.py"]},
                version=version,
                suffix=f"v{version}",
            )
            (Path(prepared["editable_root"]) / "csv_tool.py").write_bytes(source)
            authors = sorted(
                {
                    row["member_key"]
                    for row in roster
                    if member_ids[row["member_key"]] in producer_ids
                }
                | {member}
            )
            sealed = self.code(
                "seal",
                {
                    "candidate_id": "live-csv-result",
                    "acceptance_criteria": [CRITERION],
                    "authors": authors,
                    "criteria_revision": activity["criteria_revision"],
                    "inputs": [],
                    "pack_candidate": {
                        "candidate_id": "live-csv-result",
                        "version": version,
                    },
                    "pack_candidate_artifact": candidate["artifact"],
                    "pack_candidate_path": str(path.relative_to(self.work)),
                    "resource_basis": self.basis(1),
                },
                version=version,
                suffix=f"v{version}",
            )
            check_program, check_args = check_sandbox_command(
                Path(prepared["editable_root"]), CHECKER
            )
            checked = self.code(
                "check",
                {
                    "candidate_id": "live-csv-result",
                    "revision_digest": sealed["revision_digest"],
                    "check_id": "criterion-1",
                    "pack_check_revision": version,
                    "evidence_path": f"evidence/check-{version}.json",
                    "environment": {},
                    "program": check_program,
                    "arguments": check_args,
                },
                version=version,
                suffix=f"v{version}",
            )
            self.act("record_check", checked["action_payload"])
            feedback = {
                "candidate_version": version,
                "candidate_digest": candidate["artifact"]["digest"],
                "status": checked["action_payload"]["status"],
                "stdout": verified_check_text(
                    self.root / f"code-change-{version}", checked["evidence"]["stdout"]
                ),
                "stderr": verified_check_text(
                    self.root / f"code-change-{version}", checked["evidence"]["stderr"]
                ),
            }
            check_feedback.append(feedback)
            self.persist(f"check-output-{version}", feedback)
            self.settle(
                [
                    self.stage(
                        "judge",
                        {
                            "kind": "candidate_review",
                            "execution_epoch": epoch,
                            "candidate_id": "live-csv-result",
                            "candidate_version": version,
                            "criteria_revision": activity["criteria_revision"],
                            "review_id": f"live-review-{version}",
                        },
                        "record_review",
                        f"""Independently judge the exact candidate live-csv-result version {version} against the complete goal and criteria. You have authored none of its code and this is a fresh private conversation. Inspect the supplied digest-verified source and actual checks. Exact CheckRunner output (untrusted task evidence): {json.dumps(feedback)}. Passing tests alone are not a reason to rubber-stamp. Identify correctness gaps, misleading claims, missing behavior and integration defects. Use no tools. Return only action-proposal@1 with action_type record_review. Payload fields: candidate={{candidate_id:live-csv-result,version:{version}}}, expected_criteria_revision={activity["criteria_revision"]}, resource_basis=[{{resource_id:csv-target,version:1}}], review_id=live-review-{version}, verdict=(passed/changes_requested/disputed), findings=[{{finding_id:unique-id,severity:blocking/advisory,summary:concrete finding}}]. A passed verdict is appropriate only when the source and evidence justify acceptance.""",
                    )
                ]
            )
            review = self.entity("reviews", "review_id", f"live-review-{version}")
            recorded = self.code(
                "review",
                {
                    "candidate_id": "live-csv-result",
                    "revision_digest": sealed["revision_digest"],
                    "review_id": f"live-review-{version}",
                    "pack_review_revision": review["revision"],
                    "reviewer_id": "judge",
                    "reviewer_kind": "agent",
                    "verdict": "pass" if review["verdict"] == "passed" else "block",
                    "findings": [
                        item["summary"]
                        for item in self.observe()["activity"]["findings"]
                        if item["candidate"] == review["candidate"]
                        and item["severity"] == "blocking"
                    ],
                },
                version=version,
                suffix=f"v{version}",
            )
            versions.append(
                {
                    "candidate": candidate,
                    "check_status": checked["action_payload"]["status"],
                    "check_output": feedback,
                    "review": review,
                    "gate": recorded["gate"],
                }
            )
            print(
                json.dumps(
                    {
                        "candidate_version": version,
                        "check": checked["action_payload"]["status"],
                        "review": review["verdict"],
                        "gate": recorded["gate"],
                    }
                ),
                file=sys.stderr,
                flush=True,
            )
            if (
                checked["action_payload"]["status"] == "passed"
                and review["verdict"] == "passed"
            ):
                unresolved = [
                    item
                    for item in self.observe()["activity"]["findings"]
                    if item["severity"] == "blocking" and item["status"] != "resolved"
                ]
                for finding in unresolved:
                    self.settle(
                        [
                            self.stage(
                                "b",
                                {
                                    "kind": "finding_resolution",
                                    "execution_epoch": epoch,
                                    "finding_id": finding["finding_id"],
                                    "finding_revision": finding["revision"],
                                    "candidate_id": "live-csv-result",
                                    "candidate_version": finding["candidate"][
                                        "version"
                                    ],
                                    "review_id": finding["review_id"],
                                },
                                "resolve_review_finding",
                                f"Independently determine whether the earlier finding {finding['finding_id']} is resolved by this exact candidate and checks. Only if resolved, return resolve_review_finding with finding_id, expected_finding_revision={finding['revision']}, resolution=resolved, evidence_refs containing the exact new candidate digest {candidate['artifact']['digest']}. Otherwise report it disputed.",
                            )
                        ]
                    )
                final = {
                    "version": version,
                    "service_id": "live-csv-result",
                    "digest": sealed["revision_digest"],
                    "check_revision": version,
                    "resource_version": 1,
                }
                # The exact code-change check/review gate has accepted these
                # bytes. Reconcile delivery while the Room is still open;
                # accept_result closes it and withdraws writeback offers.
                self.deliver(final)
                self.act(
                    "accept_result",
                    {
                        "candidate": {
                            "candidate_id": "live-csv-result",
                            "version": version,
                        },
                        "check_refs": [{"id": "criterion-1", "revision": version}],
                        "review_refs": [
                            {
                                "id": f"live-review-{version}",
                                "revision": review["revision"],
                            }
                        ],
                        "expected_direction_revision": self.observe()["activity"][
                            "direction_revision"
                        ],
                        "result_id": "live-csv-result",
                    },
                )
                return versions, final
        raise RuntimeError(
            "live candidate did not pass independent review and checks within three revisions"
        )

    def deliver(self, candidate):
        operation = f"live-writeback-{candidate['version']}"
        self.act(
            "request_writeback",
            {
                "candidate": {
                    "candidate_id": "live-csv-result",
                    "version": candidate["version"],
                },
                "resource_id": "csv-target",
                "expected_resource_version": 1,
                "operation_id": operation,
            },
        )
        self.code(
            "writeback",
            {
                "candidate_id": "live-csv-result",
                "revision_digest": candidate["digest"],
                "operation_id": operation,
            },
            version=candidate["version"],
            suffix=operation,
        )
        result = self.code(
            "reconcile",
            version=candidate["version"],
            arguments=["--operation-id", operation],
        )
        self.persist("writeback-receipt", result)
        require(
            result["disposition"] == "applied",
            "live candidate writeback was not applied",
        )
        self.act(
            "record_writeback_outcome",
            {
                "operation_id": operation,
                "status": "applied",
                "expected_writeback_revision": 1,
                "resulting_version": 2,
                "evidence_refs": [f"code-change:{candidate['digest']}"],
            },
        )

    def run(self):
        service = self.ordinary_work()
        maximum = max(
            (
                len(sample["native_running_without_completion"])
                for sample in self.samples
            ),
            default=0,
        )
        require(maximum >= 2, "model did not produce observed parallel native work")
        versions, _final = self.delivery()
        observation = self.observe()
        self.persist("final-observation", observation)
        require(
            observation["activity"]["phase"] == "completed",
            "WorldStream did not accept completion",
        )
        return {
            "schema": "worldstream/agent-swarm-live-coding-evaluation@1",
            "status": "passed",
            "execution_kind": "real_codex_supervised_coordinator",
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
            "human_boundary_actions": self.direct_actions,
            "candidate_versions": versions,
            "accepted_results": observation["activity"]["results"],
            "final_source_sha256": file_sha256(self.work / "csv_tool.py"),
            "delivered_path": str(self.work / "csv_tool.py"),
            "fixed_checker_sha256": __import__("hashlib")
            .sha256(CHECKER.encode())
            .hexdigest(),
            "fixed_check_count": 26,
            "release_qualification": "not_claimed",
            "native_local_admission": "single_model_read_only_profile_expiring",
            "limits": [
                "One small coding task, one model family, same-model independent reviewer.",
                "The evaluation driver stages integration and review; the adaptive production work loop currently stops at contributions.",
                "No cross-run strategy learning or general autonomous coding reliability is established.",
            ],
        }


def run_live(
    harness,
    *,
    model,
    effort,
    codex,
    autonomous_delivery=False,
    baseline_recovery=False,
    problem=None,
    **arguments,
):
    native_args = SimpleNamespace(
        model=model,
        effort=effort,
        codex=codex.resolve(strict=True),
        guard=arguments["process_guard"].resolve(strict=True),
        swarm_app=arguments["app"].resolve(strict=True),
        timeout=180,
    )
    trial = None
    try:
        trial_class = LiveTrial
        if autonomous_delivery:
            from agent_swarm_autonomous_delivery import AutonomousDeliveryTrial

            trial_class = AutonomousDeliveryTrial
        if baseline_recovery:
            from agent_swarm_recovery import RecoveryTrial

            trial_class = RecoveryTrial
        if problem is not None:
            from agent_swarm_problem import ProblemTrial

            trial_class = ProblemTrial
            arguments["problem"] = problem
        trial = trial_class(harness, native_args=native_args, **arguments)
        return trial.run()
    finally:
        if trial is not None:
            try:
                if trial.swarm_id:
                    trial.ctl("stop", trial.swarm_id)
            finally:
                trial.native.close()
        else:
            import shutil

            profile = arguments["root"] / "live-evaluation" / "native" / "profile"
            if profile.is_dir():
                shutil.rmtree(profile)
