"""A deterministic coding challenge through the real managed Swarm boundaries.

The harness supplies plans and code; controlled workers exercise execution and
coordination, not model reasoning. This module never invokes a paid provider.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

CRITERION = "CSV parsing and formatting pass all 16 fixed behavioral cases."
DELAY_MS = 1500
PARSER = """import csv
import io

def parse_records(text):
    try:
        rows = list(csv.reader(io.StringIO(text, newline=""), strict=True))
    except csv.Error as error:
        raise ValueError("invalid CSV") from error
    if not rows or rows[0] != ["name", "note"]:
        raise ValueError("expected name,note header")
    if any(len(row) != 2 for row in rows[1:]):
        raise ValueError("expected two columns")
    return [dict(zip(rows[0], row)) for row in rows[1:]]
"""
FORMATTER = """import csv
import io

def format_records(records):
    output = io.StringIO(newline="")
    writer = csv.writer(output, lineterminator="\\n")
    writer.writerow(["name", "note"])
    for row in records:
        if set(row) != {"name", "note"} or not all(isinstance(value, str) for value in row.values()):
            raise ValueError("expected two string fields")
        writer.writerow([row["name"], row["note"]])
    return output.getvalue()
"""
BAD_FORMATTER = """def format_records(records):
    if any(set(row) != {"name", "note"} for row in records):
        raise ValueError("expected two fields")
    return "name,note\\n" + "".join(row["name"] + "," + row["note"] + "\\n" for row in records)
"""
HUMAN_EDIT = "# Preserve this concurrently added human note.\n"

# Passed with -c so the exact executable, argv, tests and candidate bytes are
# bound by CheckRunner evidence. No mutable external test script is consulted.
CHECKER = r'''
import json, runpy
module = runpy.run_path("csv_tool.py")
parse, fmt = module["parse_records"], module["format_records"]
checks = []
def equal(name, actual, expected):
    checks.append((name, actual == expected))
def rejects(name, function, argument):
    try:
        function(argument)
    except ValueError:
        checks.append((name, True))
    else:
        checks.append((name, False))
equal("empty_records", parse("name,note\n"), [])
equal("plain", parse("name,note\nAda,hello\n"), [{"name":"Ada","note":"hello"}])
equal("quoted_comma", parse('name,note\nAda,"a,b"\n'), [{"name":"Ada","note":"a,b"}])
equal("escaped_quote", parse('name,note\nAda,"say ""hi"""\n'), [{"name":"Ada","note":'say "hi"'}])
equal("embedded_newline", parse('name,note\nAda,"first\nsecond"\n'), [{"name":"Ada","note":"first\nsecond"}])
equal("crlf", parse("name,note\r\nAda,hello\r\n"), [{"name":"Ada","note":"hello"}])
equal("unicode", parse("name,note\nZoë,東京\n"), [{"name":"Zoë","note":"東京"}])
equal("empty_fields", parse("name,note\n,\n"), [{"name":"","note":""}])
rejects("missing_header", parse, "")
rejects("wrong_header", parse, "note,name\nhello,Ada\n")
rejects("missing_field", parse, "name,note\nAda\n")
rejects("extra_field", parse, "name,note\nAda,hello,extra\n")
rejects("unclosed_quote", parse, 'name,note\nAda,"oops')
equal("empty_format", fmt([]), "name,note\n")
rows = [{"name":"Zoë, Ada", "note":'first\n"東京"'}, {"name":"", "note":""}]
try:
    equal("round_trip", parse(fmt(rows)), rows)
except (ValueError, TypeError):
    checks.append(("round_trip", False))
rejects("invalid_format_fields", fmt, [{"name":"Ada"}])
failed = [name for name, passed in checks if not passed]
print(json.dumps({"cases":len(checks), "passed":len(checks)-len(failed), "failed":failed}))
raise SystemExit(0 if len(checks) == 16 and not failed else 1)
'''


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(f"Agent Swarm challenge: {message}")


def sha256(text: str) -> str:
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


def file_sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()


class Trial:
    def __init__(
        self,
        harness: Any,
        *,
        app: Path,
        process_guard: Path,
        execution_control: Path,
        controlled_worker: Path,
        root: Path,
        work: Path,
        local: list[str],
        execution_state: Path,
        registered_swarm_ids: list[str],
        mode: str,
        cap: int,
    ) -> None:
        self.h = harness
        self.app = app
        self.guard = process_guard
        self.control = execution_control
        self.worker = controlled_worker
        self.root = root / f"challenge-{mode}"
        self.root.mkdir()
        self.work = work / f"challenge-{mode}"
        self.work.mkdir()
        (self.work / "evidence").mkdir()
        self.local = local
        self.execution_state = execution_state
        self.mode, self.cap = mode, cap
        self.action_ids = harness.ActionIds()
        self.invocations = 0
        self.direct_actions = 0
        self.human_reconciliations = 0
        self.samples: list[dict[str, Any]] = []
        self.swarm_id = ""
        self.started = time.monotonic()
        request = {
            "goal": "Implement and validate a CSV parser and formatter in parallel.",
            "constraints": [
                "Deterministic fixture code; preserve concurrent human edits."
            ],
            "acceptance_criteria": [{"text": CRITERION}],
            "working_area": str(self.work),
            "progress_review_interval_seconds": 3600,
            "roster": [
                {
                    "member_key": key,
                    "label": label,
                    "provider": "controlled",
                    "requested_model": "fixture-v1",
                    "requested_effort": "medium",
                    "configuration_state": "fixture_unavailable",
                }
                for key, label in (
                    ("parser", "Parser"),
                    ("formatter", "Formatter"),
                    ("reviewer", "Independent reviewer"),
                )
            ],
        }
        created = self.app_command(
            "create", "--request", str(self.persist("create", request)), scoped=False
        )
        self.swarm_id = created["swarm_id"]
        self.room_id = created["room_id"]
        registered_swarm_ids.append(self.swarm_id)
        self.coordinator = [
            *local,
            "--swarm-id",
            self.swarm_id,
            "--coordinator-state",
            str(self.root / "coordinator"),
            "--controlled-path",
            str(self.worker),
        ]
        setup = self.observe()["activity"]["setup_revision"]
        self.act("confirm_initial_setup", {"setup_revision": setup})
        self.ctl("set-provider-cap", "controlled", str(cap))
        self.ctl("resume", self.swarm_id)
        self.command([str(self.app), "worker-run", *self.coordinator, "--once"])
        bootstrap = self.observe()["activity"]
        self.bootstrap_generated_work_items = len(bootstrap["work_items"])
        require(
            self.bootstrap_generated_work_items == 0,
            "no-plan bootstrap unexpectedly created work; update the autonomy measurement",
        )

    def persist(self, label: str, value: Any) -> Path:
        path = self.root / f"{label}.json"
        path.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")), encoding="utf-8"
        )
        return path

    def command(self, arguments: list[str]) -> Any:
        timeout = getattr(self, "command_timeout", 45)
        try:
            result = subprocess.run(
                arguments,
                cwd=self.root,
                check=False,
                capture_output=True,
                text=True,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(
                f"challenge command exceeded its {timeout}-second deadline"
            ) from error
        if result.returncode:
            diagnostic = result.stderr.strip().splitlines()[-2:]
            raise RuntimeError(
                f"challenge {Path(arguments[0]).name} failed: {' | '.join(diagnostic)}"
            )
        return json.loads(result.stdout)

    def app_command(self, operation: str, *arguments: str, scoped: bool = True) -> Any:
        return self.command(
            [
                str(self.app),
                operation,
                *self.local,
                *(["--swarm-id", self.swarm_id] if scoped else []),
                *arguments,
            ]
        )

    def ctl(self, *arguments: str) -> Any:
        return self.command(
            [str(self.control), "--state", str(self.execution_state), *arguments]
        )

    def native_is_running(self, invocation: str) -> bool:
        # Status retains supervisor Running until completion acknowledgment.
        # Collect polls native processes first and does not consume the result.
        # A known launched, active Invocation with no collected result is still
        # native-running; never interpret an arbitrary control failure as that.
        result = subprocess.run(
            [
                str(self.control),
                "--state",
                str(self.execution_state),
                "collect",
                invocation,
            ],
            cwd=self.root,
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode:
            require(
                result.stderr.strip() == "the requested execution object was not found",
                "native concurrency observation failed for an unexpected reason",
            )
            return True
        completion = json.loads(result.stdout)
        require(
            completion.get("kind") == "completion"
            and completion.get("value", {}).get("invocation_id") == invocation,
            "native completion observation was mismatched",
        )
        return False

    def observe(self, member: str | None = None) -> dict[str, Any]:
        return self.app_command(
            "observe", "--actor", f"worker:{member}" if member else "human"
        )

    def act(
        self, action_type: str, payload: dict[str, Any], *, expected: str = "accepted"
    ) -> dict[str, Any]:
        observation = self.observe()
        action = self.h.exact_action(
            observation, "human", action_type, self.action_ids.next(), payload
        )
        self.direct_actions += 1
        receipt = self.app_command(
            "act",
            "--request",
            str(self.persist(f"action-{self.direct_actions}", action)),
        )
        require(
            receipt.get("status") == expected,
            f"{action_type} expected {expected}, received {receipt.get('status')} ({receipt.get('code')})",
        )
        return receipt

    def entity(self, collection: str, id_field: str, identity: str) -> dict[str, Any]:
        matches = [
            item
            for item in self.observe()["activity"][collection]
            if item[id_field] == identity
        ]
        require(bool(matches), f"missing {collection} entity")
        return matches[-1]

    def work_target(self, work_id: str) -> tuple[dict[str, Any], dict[str, Any]]:
        activity = self.observe()["activity"]
        work = next(
            item for item in activity["work_items"] if item["work_id"] == work_id
        )
        attempt = next(
            item
            for item in activity["work_attempts"]
            if item["attempt_id"] == work["active_attempt_id"]
        )
        target = {
            "kind": "work_attempt",
            "execution_epoch": activity["execution_epoch"],
            "work_id": work_id,
            "work_revision": work["revision"],
            "attempt_id": attempt["attempt_id"],
            "attempt_revision": attempt["revision"],
        }
        return target, activity

    def stage(
        self,
        member: str,
        target: dict[str, Any],
        action_type: str,
        payload: dict[str, Any],
        *,
        artifact_path: str | None = None,
        artifact_text: str | None = None,
        delay_ms: int = 0,
    ) -> str:
        self.invocations += 1
        invocation = f"challenge-{self.mode}-{self.invocations}"
        observation = self.observe(member)
        roster = next(
            item
            for item in observation["activity"]["roster"]
            if item["member_key"] == member
        )
        proposal: dict[str, Any] = {
            "schema": "worldstream/agent-swarm-action-proposal@1",
            "action_type": action_type,
            "payload": payload,
        }
        if artifact_path is not None:
            proposal["artifact"] = {
                "artifact_id": f"artifact-{invocation}",
                "local_path": artifact_path,
                "media_type": "text/x-python",
            }
        instruction: Any = proposal
        if artifact_text is not None:
            instruction = {
                "schema": "worldstream/agent-swarm-controlled-artifact@1",
                "proposal": proposal,
                "artifact_text": artifact_text,
                "delay_ms": delay_ms,
            }
        plan = {
            "invocation_id": invocation,
            "swarm_id": self.swarm_id,
            "member_key": member,
            "allowed_action_types": [action_type],
            "provider": "controlled",
            "configuration_revision": roster["configuration_revision"],
            "model": roster["requested_model"],
            "effort": roster.get("requested_effort"),
            "moving_alias_acknowledged": roster["moving_alias_acknowledged"],
            "resource_policy": "workspace_write",
            "allowed_tools": [],
            "session": {"mode": "fresh", "requested_id": None},
            "kind": "work",
            "due_sequence": self.invocations,
            "semantic_target": target,
            "instruction": json.dumps(
                instruction, sort_keys=True, separators=(",", ":")
            ),
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

    def settle(self, invocations: list[str], *, measure: bool = False) -> float:
        self.settle_operation = "starting"
        try:
            return self._settle(invocations, measure=measure)
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            self.capture_failure(invocations, type(error).__name__)
            raise

    def capture_failure(self, invocations: list[str], failure_type: str) -> None:
        """Retain bounded, sanitized pre-cleanup state without masking failure."""
        try:
            snapshot: dict[str, Any] = {
                "schema": "worldstream/agent-swarm-challenge-failure/v1",
                "operation": self.settle_operation,
                "failure_type": failure_type,
                "invocation_ids": invocations,
                "coordinator_intents": {},
            }
            try:
                result = subprocess.run(
                    [
                        str(self.control),
                        "--state",
                        str(self.execution_state),
                        "status",
                        "--swarm-id",
                        self.swarm_id,
                    ],
                    capture_output=True,
                    text=True,
                    timeout=2,
                    check=False,
                )
                if result.returncode:
                    snapshot["daemon_status"] = {"capture_error": "command_failed"}
                else:
                    status = json.loads(result.stdout)["value"]
                    snapshot["daemon_status"] = {
                        "swarms": [
                            {
                                "swarm_id": swarm["swarm_id"],
                                "phase": swarm["phase"],
                                "active": [
                                    {
                                        "invocation_id": active["ticket"][
                                            "invocation_id"
                                        ],
                                        "resolution": active["resolution"],
                                    }
                                    for active in swarm["active"]
                                ],
                            }
                            for swarm in status["swarms"]
                        ],
                    }
            except (
                OSError,
                RuntimeError,
                ValueError,
                KeyError,
                subprocess.TimeoutExpired,
            ) as error:
                snapshot["daemon_status"] = {"capture_error": type(error).__name__}
            try:
                journal = self.root / "coordinator/coordinator/coordinator.json"
                if journal.stat().st_size > 16 * 1024 * 1024:
                    snapshot["coordinator_capture_error"] = "journal_too_large"
                else:
                    retained = json.loads(journal.read_text())["state"]["intents"]
                    for invocation in invocations:
                        intent = retained.get(invocation)
                        if intent is None:
                            continue
                        submission = intent.get("submission", {})
                        value = submission.get("value", {})
                        submission_view: dict[str, Any] = {
                            "status": submission.get("status")
                        }
                        if isinstance(value, dict):
                            submission_view["value"] = {
                                key: value[key]
                                for key in ("status", "code", "room_seq")
                                if key in value
                            }
                        elif isinstance(value, str):
                            submission_view["reason"] = value
                        snapshot["coordinator_intents"][invocation] = {
                            "state": intent["state"],
                            "prepared": intent.get("prepared") is not None,
                            "submission": submission_view,
                        }
            except (OSError, ValueError, KeyError) as error:
                snapshot["coordinator_capture_error"] = type(error).__name__
            self.persist("failure-diagnostics", snapshot)
        except (
            OSError,
            RuntimeError,
            ValueError,
            KeyError,
            TypeError,
            AttributeError,
            subprocess.TimeoutExpired,
        ):
            # Diagnostics must never replace the original execution failure.
            return

    def _settle(self, invocations: list[str], *, measure: bool = False) -> float:
        started = time.monotonic()
        deadline = started + 45
        pending = set(invocations)
        launched: set[str] = set()
        while time.monotonic() < deadline:
            self.settle_operation = "worker-dispatch"
            events = self.command([str(self.app), "worker-dispatch", *self.coordinator])
            launched.update(
                event["invocation_id"]
                for event in events
                if event.get("event") == "launched"
            )
            self.settle_operation = "daemon-status"
            status = self.ctl("status", "--swarm-id", self.swarm_id)["value"]
            active = [
                entry["ticket"]["invocation_id"]
                for swarm in status["swarms"]
                for entry in swarm["active"]
                if entry["resolution"] == "running"
                and entry["ticket"]["invocation_id"] in invocations
            ]
            require(len(active) <= self.cap, "provider cap exceeded")
            if measure:
                self.settle_operation = "native-overlap-sample"
                native_running = sorted(
                    invocation
                    for invocation in set(active) & launched
                    if self.native_is_running(invocation)
                )
                self.samples.append(
                    {
                        "elapsed_ms": round((time.monotonic() - started) * 1000),
                        "running": sorted(active),
                        "launched_and_running": sorted(set(active) & launched),
                        "native_running_without_completion": native_running,
                    }
                )
            self.settle_operation = "worker-harvest"
            self.command([str(self.app), "worker-harvest", *self.coordinator])
            for invocation in sorted(pending):
                self.settle_operation = "worker-intent"
                intent = self.command(
                    [
                        str(self.app),
                        "worker-intent",
                        *self.coordinator,
                        "--invocation-id",
                        invocation,
                    ]
                )
                require(
                    intent["state"] != "needs_reevaluation",
                    f"{invocation} required reevaluation",
                )
                if intent["state"] == "settled":
                    submission = intent["submission"]
                    require(
                        submission.get("status") == "received"
                        and submission.get("value", {}).get("status") == "accepted",
                        f"{invocation} settled without an accepted Action",
                    )
                    pending.remove(invocation)
            if not pending:
                return time.monotonic() - started
            time.sleep(0.04)
        raise RuntimeError("challenge coordinator failed to settle within 45 seconds")

    def claim(self, member: str, work_id: str) -> None:
        activity = self.observe()["activity"]
        work = next(
            item for item in activity["work_items"] if item["work_id"] == work_id
        )
        self.settle(
            [
                self.stage(
                    member,
                    {
                        "kind": "work_claim",
                        "execution_epoch": activity["execution_epoch"],
                        "work_id": work_id,
                        "work_revision": work["revision"],
                    },
                    "claim_work_item",
                    {
                        "attempt_id": f"attempt-{work_id}",
                        "expected_work_revision": work["revision"],
                        "work_id": work_id,
                    },
                )
            ]
        )

    def contribution(
        self,
        member: str,
        work_id: str,
        label: str,
        source: str,
        *,
        complete: bool,
        delay_ms: int = 0,
        resource_version: int | None = None,
    ) -> str:
        target, _ = self.work_target(work_id)
        return self.stage(
            member,
            target,
            "submit_contribution",
            {
                "contribution_id": label,
                "completes_work": complete,
                "expected_attempt_revision": target["attempt_revision"],
                "expected_work_revision": target["work_revision"],
                "work_id": work_id,
                "resource_basis": self.basis(resource_version),
                "source_refs": ["controlled:csv-challenge"],
                "summary": "Deterministic CSV source contribution.",
            },
            artifact_path=f"contributions/{label}.py",
            artifact_text=source,
            delay_ms=delay_ms,
        )

    @staticmethod
    def basis(version: int | None) -> list[dict[str, Any]]:
        return (
            []
            if version is None
            else [{"resource_id": "csv-target", "version": version}]
        )

    def code(
        self,
        operation: str,
        value: dict[str, Any] | None = None,
        *,
        version: int,
        suffix: str = "",
        arguments: list[str] | None = None,
    ) -> dict[str, Any]:
        command = [
            str(self.app),
            "code-change",
            "--working-area",
            str(self.work),
            "--code-change-state",
            str(self.root / f"code-change-{version}"),
            operation,
        ]
        if value is not None:
            command += [
                "--request",
                str(self.persist(f"code-{operation}-{suffix}", value)),
            ]
        if operation == "check":
            command += ["--process-guard", str(self.guard)]
        return self.command([*command, *(arguments or [])])

    def candidate(
        self, version: int, source: str, resource_version: int, expected_check: str
    ) -> dict[str, Any]:
        label = f"integration-{version}"
        self.settle(
            [
                self.contribution(
                    "parser",
                    "integration",
                    label,
                    source,
                    complete=False,
                    resource_version=resource_version,
                )
            ]
        )
        target, activity = self.work_target("integration")
        path = f"candidates/csv-{version}.py"
        contribution_refs = [
            {"contribution_id": item, "version": 1}
            for item in ("parser-source", "formatter-source", label)
        ]
        self.settle(
            [
                self.stage(
                    "parser",
                    target,
                    "submit_candidate",
                    {
                        "candidate_id": "csv-result",
                        "contribution_refs": contribution_refs,
                        "expected_work_revision": target["work_revision"],
                        "work_id": "integration",
                        "resource_basis": self.basis(resource_version),
                    },
                    artifact_path=path,
                    artifact_text=source,
                )
            ]
        )
        candidate = self.entity("candidates", "candidate_id", "csv-result")
        require(candidate["version"] == version, "candidate version did not advance")
        candidate_bytes = (self.work / path).read_bytes()
        require(
            candidate_bytes == source.encode(),
            "worker candidate bytes differ from the requested fixture",
        )
        service_id = "csv-result"
        prepared = self.code(
            "prepare",
            {"candidate_id": service_id, "targets": ["csv_tool.py"]},
            version=version,
            suffix=label,
        )
        (Path(prepared["editable_root"]) / "csv_tool.py").write_bytes(candidate_bytes)
        sealed = self.code(
            "seal",
            {
                "candidate_id": service_id,
                "acceptance_criteria": [CRITERION],
                "authors": ["parser", "formatter"],
                "criteria_revision": activity["criteria_revision"],
                "inputs": [],
                "pack_candidate": {"candidate_id": "csv-result", "version": version},
                "pack_candidate_artifact": candidate["artifact"],
                "pack_candidate_path": path,
                "resource_basis": self.basis(resource_version),
            },
            version=version,
            suffix=label,
        )
        checked = self.code(
            "check",
            {
                "candidate_id": service_id,
                "revision_digest": sealed["revision_digest"],
                "check_id": "criterion-1",
                "pack_check_revision": version,
                "evidence_path": f"evidence/check-{version}.json",
                "environment": {},
                "program": str(Path(sys.executable).resolve()),
                "arguments": ["-B", "-c", CHECKER],
            },
            version=version,
            suffix=label,
        )
        check_payload = checked["action_payload"]
        require(
            check_payload["status"] == expected_check,
            f"candidate {version} check had wrong outcome",
        )
        self.act("record_check", check_payload)
        verdict = "changes_requested" if expected_check == "failed" else "passed"
        findings = (
            [
                {
                    "finding_id": "bad-csv-escaping",
                    "severity": "blocking",
                    "summary": "The controlled formatter fails the deterministic CSV cases.",
                }
            ]
            if expected_check == "failed"
            else []
        )
        self.settle(
            [
                self.stage(
                    "reviewer",
                    {
                        "kind": "candidate_review",
                        "execution_epoch": activity["execution_epoch"],
                        "candidate_id": "csv-result",
                        "candidate_version": version,
                        "criteria_revision": activity["criteria_revision"],
                        "review_id": f"review-{version}",
                    },
                    "record_review",
                    {
                        "candidate": {"candidate_id": "csv-result", "version": version},
                        "expected_criteria_revision": activity["criteria_revision"],
                        "resource_basis": self.basis(resource_version),
                        "review_id": f"review-{version}",
                        "verdict": verdict,
                        "findings": findings,
                    },
                )
            ]
        )
        review = self.code(
            "review",
            {
                "candidate_id": service_id,
                "revision_digest": sealed["revision_digest"],
                "review_id": f"review-{version}",
                "pack_review_revision": 1,
                "reviewer_id": "reviewer",
                "reviewer_kind": "agent",
                "verdict": "block" if expected_check == "failed" else "pass",
                "findings": ["CSV escaping is incorrect"]
                if expected_check == "failed"
                else [],
            },
            version=version,
            suffix=label,
        )
        require(
            review["gate"]
            == ("failed_check" if expected_check == "failed" else "accepted"),
            "durable review gate did not match exact check evidence",
        )
        return {
            "version": version,
            "service_id": service_id,
            "digest": sealed["revision_digest"],
            "check_revision": version,
            "resource_version": resource_version,
        }

    def acceptance(self, candidate: dict[str, Any], *, expected: str) -> dict[str, Any]:
        return self.act(
            "accept_result",
            {
                "candidate": {
                    "candidate_id": "csv-result",
                    "version": candidate["version"],
                },
                "check_refs": [
                    {"id": "criterion-1", "revision": candidate["check_revision"]}
                ],
                "review_refs": [
                    {"id": f"review-{candidate['version']}", "revision": 1}
                ],
                "expected_direction_revision": self.observe()["activity"][
                    "direction_revision"
                ],
                "result_id": "csv-result",
            },
            expected=expected,
        )

    def writeback(self, candidate: dict[str, Any], *, conflict: bool) -> str:
        operation = f"writeback-{candidate['version']}"
        self.act(
            "request_writeback",
            {
                "candidate": {
                    "candidate_id": "csv-result",
                    "version": candidate["version"],
                },
                "resource_id": "csv-target",
                "expected_resource_version": candidate["resource_version"],
                "operation_id": operation,
            },
        )
        self.code(
            "writeback",
            {
                "candidate_id": candidate["service_id"],
                "revision_digest": candidate["digest"],
                "operation_id": operation,
            },
            version=candidate["version"],
            suffix=operation,
        )
        if conflict:
            (self.work / "csv_tool.py").write_text(HUMAN_EDIT, encoding="utf-8")
        result = self.code(
            "reconcile",
            version=candidate["version"],
            arguments=["--operation-id", operation],
        )
        require(
            result["disposition"] == ("blocked_conflict" if conflict else "applied"),
            "write-back outcome mismatch",
        )
        if conflict:
            require(
                (self.work / "csv_tool.py").read_text() == HUMAN_EDIT,
                "concurrent human bytes were overwritten",
            )
        self.act(
            "record_writeback_outcome",
            {
                "operation_id": operation,
                "expected_writeback_revision": 1,
                "status": "conflicted" if conflict else "applied",
                "resulting_version": candidate["resource_version"]
                if conflict
                else candidate["resource_version"] + 1,
                "evidence_refs": [f"code-change:{candidate['digest']}"],
            },
        )
        return operation

    def run(self) -> dict[str, Any]:
        for work_id, dependencies in (
            ("parser", []),
            ("formatter", []),
            ("integration", ["parser", "formatter"]),
        ):
            self.act(
                "propose_work_item",
                {
                    "work_id": work_id,
                    "dependency_ids": dependencies,
                    "description": f"Controlled CSV {work_id} work.",
                    "expected_goal_revision": 1,
                    "kind": "integration" if work_id == "integration" else "goal",
                    "title": work_id,
                },
            )
        # Test dependency legality through the actual Pack using its controlled
        # fixture-only direct Action seam; all successful worker work uses plans.
        observation = self.observe("parser")
        early = self.h.exact_action(
            observation,
            "worker:parser",
            "claim_work_item",
            self.action_ids.next(),
            {
                "attempt_id": "too-early",
                "expected_work_revision": 1,
                "work_id": "integration",
            },
        )
        premature = self.app_command(
            "act",
            "--request",
            str(self.persist("premature", early)),
            "--allow-controlled-worker-fixture",
        )
        require(
            premature.get("status") == "rejected"
            and premature.get("code") == "work_ineligible",
            "dependency gate allowed early integration",
        )
        self.claim("parser", "parser")
        self.claim("formatter", "formatter")
        turns = [
            self.contribution(
                "parser",
                "parser",
                "parser-source",
                PARSER,
                complete=True,
                delay_ms=DELAY_MS,
            ),
            self.contribution(
                "formatter",
                "formatter",
                "formatter-source",
                FORMATTER,
                complete=True,
                delay_ms=DELAY_MS,
            ),
        ]
        coding_seconds = self.settle(turns, measure=True)
        max_parallel = max(
            len(sample["native_running_without_completion"]) for sample in self.samples
        )
        require(
            max_parallel == self.cap,
            f"expected {self.cap} overlapping launched workers, observed {max_parallel}",
        )
        require(
            (self.work / "contributions/parser-source.py").read_text() == PARSER,
            "parser artifact mismatch",
        )
        require(
            (self.work / "contributions/formatter-source.py").read_text() == FORMATTER,
            "formatter artifact mismatch",
        )
        self.claim("parser", "integration")
        (self.work / "csv_tool.py").write_text("# original target\n", encoding="utf-8")
        self.act(
            "record_resource_version",
            {
                "resource_id": "csv-target",
                "version": 1,
                "expected_previous_version": 0,
                "local_path": str(self.work / "csv_tool.py"),
                "digest": sha256("# original target\n"),
                "affected_work_ids": ["integration"],
            },
        )
        parser_source = (self.work / "contributions/parser-source.py").read_text()
        formatter_source = (self.work / "contributions/formatter-source.py").read_text()
        bad = self.candidate(1, parser_source + "\n" + BAD_FORMATTER, 1, "failed")
        bad_rejection = self.acceptance(bad, expected="rejected")
        require(
            bad_rejection.get("code") == "validation_incomplete",
            "failed CSV checks did not block acceptance",
        )
        good_source = parser_source + "\n" + formatter_source
        good = self.candidate(2, good_source, 1, "passed")
        retained_finding = self.acceptance(good, expected="rejected")
        require(
            retained_finding.get("code") == "blocking_finding",
            "a later pass erased the prior blocking finding",
        )
        self.act(
            "resolve_review_finding",
            {
                "finding_id": "bad-csv-escaping",
                "expected_finding_revision": 1,
                "resolution": "resolved",
                "evidence_refs": ["check:criterion-1:2"],
            },
        )
        self.human_reconciliations += 1
        operation = self.writeback(good, conflict=True)
        conflict_rejection = self.acceptance(good, expected="rejected")
        require(
            conflict_rejection.get("code") == "resource_conflict",
            "write-back conflict did not block result",
        )
        self.act(
            "record_resource_version",
            {
                "resource_id": "csv-target",
                "version": 2,
                "expected_previous_version": 1,
                "local_path": str(self.work / "csv_tool.py"),
                "digest": sha256(HUMAN_EDIT),
                "affected_work_ids": ["integration"],
            },
        )
        stale = self.acceptance(good, expected="rejected")
        require(
            stale.get("code") == "stale_resource_basis",
            "old candidate survived changed resource basis",
        )
        self.act(
            "resolve_work_blocker",
            {
                "blocker_id": f"writeback-conflict:{operation}",
                "expected_blocker_revision": 1,
                "evidence_refs": ["resource:csv-target:2"],
            },
        )
        self.human_reconciliations += 1
        final_source = HUMAN_EDIT + good_source
        final = self.candidate(3, final_source, 2, "passed")
        self.writeback(final, conflict=False)
        self.acceptance(final, expected="accepted")
        final_observation = self.observe()
        activity = final_observation["activity"]
        require(
            activity["phase"] == "completed" and len(activity["results"]) == 1,
            "Room did not retain exactly one accepted result",
        )
        require(
            activity["results"][0]["candidate"]
            == {"candidate_id": "csv-result", "version": 3},
            "wrong candidate was accepted",
        )
        require(
            (self.work / "csv_tool.py").read_text() == final_source,
            "final write-back lost source or human note",
        )
        self.ctl("stop", self.swarm_id)
        final_candidate = next(
            item
            for item in activity["candidates"]
            if item["candidate_id"] == "csv-result" and item["version"] == 3
        )
        return {
            "mode": self.mode,
            "provider_cap": self.cap,
            "room_id": self.room_id,
            "swarm_id": self.swarm_id,
            "final_room_seq": final_observation["room_seq"],
            "final_authoritative_state_hash": final_observation[
                "authoritative_state_hash"
            ],
            "accepted_result": activity["results"][0],
            "final_candidate_artifact": {
                key: final_candidate["artifact"][key]
                for key in ("artifact_id", "digest", "media_type")
            },
            "sealed_revision_digest": final["digest"],
            "coding_seconds": round(coding_seconds, 3),
            "total_seconds": round(time.monotonic() - self.started, 3),
            "max_observed_launched_running": max_parallel,
            "concurrency_samples": self.samples,
            "overlap_evidence": "launched plus supervisor-active plus native Collect returning NotFound before harvest",
            "final_source_sha256": sha256(final_source),
            "cases_passed": 16,
            "candidate_versions": 3,
            "accepted_candidate_version": 3,
            "manually_staged_plan_count": self.invocations,
            "bootstrap_generated_work_items": self.bootstrap_generated_work_items,
            "scripted_human_action_count": self.direct_actions,
            "scripted_human_reconciliations": self.human_reconciliations,
            "controlled_fixture_direct_actions": 1,
            "unexpected_interventions": 0,
            "faults": {
                "premature_integration_rejected": True,
                "bad_candidate_rejected": True,
                "independent_review_requested_changes": True,
                "blocking_finding_survived_passing_review": True,
                "concurrent_edit_preserved": True,
                "conflict_blocked_acceptance": True,
                "stale_candidate_rejected": True,
                "revalidated_candidate_accepted": True,
            },
        }


def run_challenge(harness: Any, **arguments: Any) -> dict[str, Any]:
    local = arguments["local"]
    identities = {
        "pack_revision_digest": local[local.index("--pack-digest") + 1],
        "application_sha256": file_sha256(arguments["app"]),
        "controlled_worker_sha256": file_sha256(arguments["controlled_worker"]),
        "process_guard_sha256": file_sha256(arguments["process_guard"]),
        "challenge_script_sha256": file_sha256(Path(__file__)),
        "checker_source_sha256": sha256(CHECKER),
    }
    trials = [
        Trial(harness, **arguments, mode=mode, cap=cap).run()
        for mode, cap in (("serial", 1), ("parallel", 2))
    ]
    require(
        trials[0]["final_source_sha256"] == trials[1]["final_source_sha256"],
        "serial and parallel results differ",
    )
    return {
        "schema": "worldstream/agent-swarm-coding-challenge/v1",
        "status": "passed",
        "identities": identities,
        "execution_kind": "controlled_deterministic_workers",
        "backend": "managed_local",
        "real_model_reasoning_tested": False,
        "autonomous_goal_planning_tested": False,
        "review_kind": "scripted_independent_roster_review",
        "synthetic_delay_ms_per_coding_turn": DELAY_MS,
        "controlled_delay_ms": DELAY_MS,
        "timing_scope": "two already-claimed coding WorkAttempts through accepted contributions",
        "timing_note": "One trial per cap; synthetic execution overlap, not model coding speedup. No ratio threshold.",
        "coding_time_ratio_serial_over_parallel": round(
            trials[0]["coding_seconds"] / trials[1]["coding_seconds"], 3
        ),
        "equal_final_artifact": True,
        "trials": trials,
    }
