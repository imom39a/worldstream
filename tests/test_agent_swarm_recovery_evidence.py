"""Regressions for offline autonomous delivery recovery evidence."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_recovery_evidence",
    Path(__file__).resolve().parents[1] / "scripts/agent_swarm_recovery_evidence.py",
)
assert SPEC is not None and SPEC.loader is not None
RECOVERY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RECOVERY)


def digest(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def artifact(artifact_id: str, data: bytes) -> dict[str, object]:
    return {
        "artifact_id": artifact_id,
        "digest": digest(data),
        "media_type": "text/x-python",
    }


def fixture():
    baseline = b"def parse(value): return value.split(',')\n"
    repaired = b"def parse(value): return value.split(',')  # repaired\n"
    criteria = b'["fixed checker passes all cases"]'
    checker_command = {
        "program": "/usr/bin/python",
        "arguments": ["-c", "fixed"],
        "environment": {},
    }
    checker_identity = {
        "executable_path": "/usr/bin/python",
        "executable": {"digest": "sha256:" + "a" * 64},
        "arguments": ["-c", "fixed"],
        "environment": {},
    }
    candidates = []
    report_versions = []
    journal_versions = {}
    evidence = {}
    outputs = {}
    for version, source, status, stdout_text in [
        (
            1,
            baseline,
            "failed",
            '{"cases":26,"passed":25,"failed":["quote_in_unquoted_field"]}\n',
        ),
        (2, repaired, "passed", '{"cases":26,"passed":26,"failed":[]}\n'),
    ]:
        candidate = {
            "candidate_id": "result-test",
            "version": version,
            "author_member_id": f"author-{version}",
            "artifact": artifact(f"candidate-{version}", source),
            "work_id": "work-repair" if version == 2 else "work-baseline",
            "work_revision": 2,
            "resource_basis": [{"resource_id": "target", "version": 1}],
            "contribution_refs": [{"contribution_id": "contrib-v2", "version": 1}]
            if version == 2
            else [],
        }
        candidates.append(candidate)
        stdout = stdout_text.encode()
        stderr = b""
        stdout_ref = {"digest": digest(stdout), "byte_length": len(stdout)}
        stderr_ref = {"digest": digest(stderr), "byte_length": len(stderr)}
        check_json = json.dumps(
            {
                "check_id": "criterion-1",
                "binding": {
                    "candidate": f"local-candidate-{version}",
                    "criteria": {
                        "digest": digest(criteria),
                        "byte_length": len(criteria),
                    },
                    "inputs": [],
                },
                "checker": checker_identity,
                "outcome": {"outcome": "failed", "exit_code": 1}
                if status == "failed"
                else {"outcome": "passed"},
                "stdout": stdout_ref,
                "stderr": stderr_ref,
            },
            sort_keys=True,
        ).encode()
        check_artifact = {
            "artifact_id": f"check-{version}",
            "digest": digest(check_json),
        }
        room_check = {
            "candidate": {"candidate_id": "result-test", "version": version},
            "check_id": "criterion-1",
            "criteria_revision": 1,
            "revision": version,
            "status": status,
            "criterion": "fixed checker passes all cases",
            "resource_basis": [{"resource_id": "target", "version": 1}],
            "recorded_by_member_id": "host",
            "evidence_refs": [
                {
                    "artifact": check_artifact,
                    "candidate": {"candidate_id": "result-test", "version": version},
                    "candidate_artifact": candidate["artifact"],
                    "check_id": "criterion-1",
                    "criteria_revision": 1,
                    "resource_basis": [{"resource_id": "target", "version": 1}],
                }
            ],
        }
        report_versions.append(
            {
                "candidate": candidate,
                "checks": [
                    {"room_check": room_check, "stdout": stdout_text, "stderr": ""}
                ],
                "reviews": [],
            }
        )
        evidence[digest(check_json)] = check_json
        outputs[digest(stdout)] = stdout
        outputs[digest(stderr)] = stderr
        journal_versions[str(version)] = {
            "sealed": {"candidate_digest": f"local-candidate-{version}"}
        }

    review = {
        "candidate": {"candidate_id": "result-test", "version": 2},
        "review_id": "review-v2",
        "revision": 1,
        "reviewer_member_id": "judge",
        "verdict": "passed",
        "criteria_revision": 1,
    }
    accepted = {
        "candidate": {"candidate_id": "result-test", "version": 2},
        "result_id": "result-v2",
        "criteria_revision": 1,
        "resource_basis": [{"resource_id": "target", "version": 1}],
        "check_refs": [{"id": "criterion-1", "revision": 2}],
        "review_refs": [{"id": "review-v2", "revision": 1}],
    }
    report = {
        "status": "passed",
        "baseline_first_instructed": True,
        "baseline_source_sha256": "sha256:" + hashlib.sha256(baseline).hexdigest(),
        "baseline_checker": {
            "exit_code": 1,
            "output": {
                "cases": 26,
                "passed": 25,
                "failed": ["quote_in_unquoted_field"],
            },
            "stdout": '{"cases":26,"passed":25,"failed":["quote_in_unquoted_field"]}\n',
            "stderr": "",
            "checker_sha256": hashlib.sha256(b"fixed-checker").hexdigest(),
            "checker": checker_command,
        },
        "fixed_checker_sha256": hashlib.sha256(b"fixed-checker").hexdigest(),
        "final_source_sha256": "sha256:" + hashlib.sha256(repaired).hexdigest(),
        "delivery_plans_staged_by_evaluation_driver": 0,
        "evaluation_driver_post_setup_actions": 0,
        "candidate_versions": report_versions,
        "accepted_results": [accepted],
    }
    observation = {
        "activity": {
            "candidates": candidates,
            "checks": [item["checks"][0]["room_check"] for item in report_versions],
            "results": [accepted],
            "reviews": [review],
            "contributions": [],
        }
    }
    declaration = {
        "schema": "worldstream/agent-swarm-recovery-declaration@1",
        "baseline_first_instructed": True,
        "baseline_source_sha256": "sha256:" + hashlib.sha256(baseline).hexdigest(),
        "baseline_source_byte_length": len(baseline),
        "acceptance_criteria": ["fixed checker passes all cases"],
        "criteria_bytes": criteria.decode(),
        "criteria_digest": digest(criteria),
        "criteria_revision": 1,
        "required_check_ids": ["criterion-1"],
        "fixed_check_count": 26,
        "checker": checker_command,
        "checker_executable_sha256": "sha256:" + "a" * 64,
        "fixed_checker_sha256": hashlib.sha256(b"fixed-checker").hexdigest(),
        "policy": {
            "delivery": {"resource_id": "target", "expected_resource_version": 1}
        },
        "preflight": report["baseline_checker"],
    }
    observation["activity"]["criteria_revision"] = 1
    observation["activity"]["contributions"] = [
        {
            "contribution_id": "contrib-v2",
            "version": 1,
            "author_member_id": "author-2",
        }
    ]

    def intent(
        invocation,
        member_id,
        target,
        action,
        proposal,
        value,
        text,
        session,
        instruction,
        room_seq,
    ):
        final_text = json.dumps(text, separators=(",", ":"))
        envelope = json.dumps(
            {
                "schema": "worldstream/codex-app-server-turn@1",
                "configuration": {
                    "model": "fixture-model",
                    "effort": "low",
                    "provider_session_id": session,
                },
                "completion": {
                    "turn": {
                        "status": "completed",
                        "error": None,
                        "items": [{"text": final_text}],
                    }
                },
            },
            sort_keys=True,
        ).encode()
        return {
            "plan": {
                "semantic_target": target,
                "session": {"mode": "fresh", "requested_id": None},
                "model": "fixture-model",
                "effort": "low",
                "instruction": instruction,
            },
            "member_id": member_id,
            "proposal": proposal,
            "action": action,
            "submission": {
                "status": "received",
                "value": {"status": "accepted", "room_seq": room_seq},
            },
            "evidence": {
                "success": True,
                "exit_code": 0,
                "stdout": {"digest": digest(envelope), "byte_length": len(envelope)},
                "output": {
                    "reported_model": "fixture-model",
                    "reported_effort": "low",
                    "session_id": session,
                    "text": final_text,
                },
            },
            "_native_stdout": envelope,
        }

    failed_stdout = report_versions[0]["checks"][0]["stdout"]
    instructions = json.dumps(
        {
            "action_contract": json.dumps(
                {
                    "task": "integrate",
                    "candidate_id": "result-test",
                    "next_version": 2,
                    "target": {
                        "kind": "work_attempt",
                        "work_id": "work-repair",
                        "work_revision": 2,
                    },
                    "contribution_refs": [
                        {"contribution_id": "contrib-v2", "version": 1}
                    ],
                    "resource_basis": [{"resource_id": "target", "version": 1}],
                    "check_feedback": {
                        "checks": [
                            {
                                "candidate_version": 1,
                                "check_id": "criterion-1",
                                "status": "failed",
                                "stdout": failed_stdout,
                            }
                        ]
                    },
                }
            )
        }
    )
    candidate_text = repaired.decode()
    candidate_intent = intent(
        "submit-v2",
        "author-2",
        {"kind": "work_attempt", "work_id": "work-repair"},
        {
            "action_type": "submit_candidate",
            "payload": {
                "candidate_id": "result-test",
                "work_id": "work-repair",
                "expected_work_revision": 2,
                "contribution_refs": [{"contribution_id": "contrib-v2", "version": 1}],
                "resource_basis": [{"resource_id": "target", "version": 1}],
                "artifact": candidates[1]["artifact"],
            },
        },
        {
            "schema": "worldstream/agent-swarm-worker-proposal@1",
            "action_type": "submit_candidate",
            "payload": {"candidate_id": "result-test"},
            "artifact": {"inline_text": candidate_text},
        },
        {"status": "accepted"},
        {
            "schema": "worldstream/agent-swarm-worker-proposal@1",
            "action_type": "submit_candidate",
            "payload": {"candidate_id": "result-test"},
            "artifact": {"inline_text": candidate_text},
        },
        "author-session",
        instructions,
        11,
    )
    candidate_intent["submission"]["value"]["candidate"] = {
        "candidate_id": "result-test",
        "version": 2,
    }
    review_text = {
        "schema": "worldstream/agent-swarm-worker-proposal@1",
        "action_type": "record_review",
        "payload": {
            "candidate": {"candidate_id": "result-test", "version": 2},
            "review_id": "review-v2",
            "verdict": "passed",
        },
    }
    review_intent = intent(
        "review-v2",
        "judge",
        {
            "kind": "candidate_review",
            "candidate_id": "result-test",
            "candidate_version": 2,
            "review_id": "review-v2",
            "criteria_revision": 1,
        },
        {
            "action_type": "record_review",
            "payload": {
                **review_text["payload"],
                "review_id": "review-v2",
            },
        },
        review_text,
        {"status": "accepted"},
        review_text,
        "judge-session",
        "judge exact Candidate",
        13,
    )
    review_intent["submission"]["value"]["review_id"] = "review-v2"
    native_stdout = {
        key: {
            "bytes": value["_native_stdout"],
            "digest": digest(value["_native_stdout"]),
            "byte_length": len(value["_native_stdout"]),
        }
        for key, value in [
            ("submit-v2", candidate_intent),
            ("review-v2", review_intent),
        ]
    }
    for intent_value in (candidate_intent, review_intent):
        intent_value.pop("_native_stdout")
    selected_bytes = json.dumps(
        {
            "schema": "worldstream/agent-swarm-recovery-provenance@1",
            "intents": {"submit-v2": candidate_intent, "review-v2": review_intent},
        },
        sort_keys=True,
    ).encode()
    delivery_actions_bytes = json.dumps(
        {
            "schema": "worldstream/agent-swarm-recovery-delivery-actions@1",
            "operations": {
                "op": {
                    "actions": [
                        {
                            "action": {
                                "action_type": "record_check",
                                "payload": {
                                    "candidate": {"version": 1},
                                    "status": "failed",
                                },
                            },
                            "receipt": {"status": "accepted", "room_seq": 10},
                        }
                    ]
                }
            },
        },
        sort_keys=True,
    ).encode()
    provenance = {
        "selected_intents_bytes": selected_bytes,
        "selected_intents_digest": digest(selected_bytes),
        "native_stdout_by_invocation": native_stdout,
        "delivery_actions_bytes": delivery_actions_bytes,
        "delivery_actions_digest": digest(delivery_actions_bytes),
    }
    return {
        "report": report,
        "observation": observation,
        "journal": {
            "versions": journal_versions,
            "executables": {"/usr/bin/python": "sha256:" + "a" * 64},
        },
        "declaration": declaration,
        "candidate_bytes": {"result-test:1": baseline, "result-test:2": repaired},
        "evidence": evidence,
        "outputs": outputs,
        "criteria": {digest(criteria): criteria},
        "provenance": provenance,
    }


def verify(data):
    return RECOVERY.verify_recovery(
        data["report"],
        data["observation"],
        data["journal"],
        data["declaration"],
        data["candidate_bytes"],
        data["evidence"],
        data["outputs"],
        data["criteria"],
        data["provenance"],
    )


def update_selected(data, mutate):
    selected = json.loads(data["provenance"]["selected_intents_bytes"])
    mutate(selected["intents"])
    encoded = json.dumps(selected, sort_keys=True).encode()
    data["provenance"]["selected_intents_bytes"] = encoded
    data["provenance"]["selected_intents_digest"] = digest(encoded)


class AutonomousRecoveryEvidence(unittest.TestCase):
    def test_real_failed_baseline_then_repaired_candidate_is_accepted(self):
        data = fixture()
        selected = json.loads(data["provenance"]["selected_intents_bytes"])
        submitted = selected["intents"]["submit-v2"]["action"]["payload"]
        self.assertNotIn("version", submitted)
        self.assertEqual(submitted["contribution_refs"][0]["version"], 1)
        self.assertEqual(data["observation"]["activity"]["candidates"][1]["version"], 2)
        result = verify(data)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["failed_check_ids"], ["criterion-1"])
        self.assertEqual(result["repaired_candidate"]["version"], 2)
        self.assertEqual(
            result["model_authorship_provenance"],
            "verified_from_retained_intents_and_native_output",
        )

    def test_wrong_worker_contract_candidate_version_is_rejected(self):
        data = fixture()

        def change_version(intents):
            instruction = json.loads(intents["submit-v2"]["plan"]["instruction"])
            contract = json.loads(instruction["action_contract"])
            contract["next_version"] = 1
            instruction["action_contract"] = json.dumps(contract)
            intents["submit-v2"]["plan"]["instruction"] = json.dumps(instruction)

        update_selected(data, change_version)
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "authorize the repaired Candidate version"
        ):
            verify(data)

    def test_prior_single_version_successful_run_is_not_recovery(self):
        root = (
            Path(__file__).resolve().parents[1]
            / "docs/evidence/agent-swarm-autonomous-delivery/results.json"
        )
        old_report = json.loads(root.read_text(encoding="utf-8"))
        data = fixture()
        data["report"] = old_report
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "at least two|baseline"
        ):
            verify(data)

    def test_first_candidate_success_is_not_recovery(self):
        data = fixture()
        data["report"]["candidate_versions"][0]["checks"][0]["room_check"]["status"] = (
            "passed"
        )
        data["observation"]["activity"]["checks"][0]["status"] = "passed"
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "Room marks a check passed"
        ):
            verify(data)

    def test_checker_change_is_rejected(self):
        data = fixture()
        data["declaration"]["checker"]["arguments"] = ["-c", "different"]
        data["declaration"]["preflight"]["checker"]["arguments"] = ["-c", "different"]
        data["report"]["baseline_checker"]["checker"]["arguments"] = ["-c", "different"]
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "checker identity changed"
        ):
            # The evidence document still identifies the original executable command.
            verify(data)

    def test_criteria_change_is_rejected(self):
        data = fixture()
        data["declaration"]["criteria_bytes"] = "changed criteria"
        with self.assertRaisesRegex(RECOVERY.RecoveryEvidenceError, "criteria"):
            verify(data)

    def test_stale_candidate_revision_evidence_is_rejected(self):
        data = fixture()
        data["journal"]["versions"]["2"]["sealed"]["candidate_digest"] = (
            "other-candidate"
        )
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "different sealed Candidate"
        ):
            verify(data)

    def test_unchanged_candidate_bytes_are_rejected(self):
        data = fixture()
        data["candidate_bytes"]["result-test:2"] = data["candidate_bytes"][
            "result-test:1"
        ]
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "do not match|unchanged"
        ):
            verify(data)

    def test_result_accepting_wrong_candidate_is_rejected(self):
        data = fixture()
        wrong = {"candidate_id": "result-test", "version": 1}
        data["report"]["accepted_results"][0]["candidate"] = wrong
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "failed baseline|other than the repaired"
        ):
            verify(data)

    def test_missing_fresh_independent_review_is_rejected(self):
        data = fixture()
        update_selected(
            data,
            lambda intents: intents["review-v2"]["plan"].__setitem__(
                "session", {"mode": "resume", "requested_id": "author-session"}
            ),
        )
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "fresh independent"
        ):
            verify(data)

    def test_evaluator_intervention_is_rejected(self):
        data = fixture()
        data["report"]["delivery_plans_staged_by_evaluation_driver"] = 1
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "evaluation driver"
        ):
            verify(data)

    def test_authorship_provenance_is_mandatory(self):
        data = fixture()
        update_selected(data, lambda intents: intents.pop("submit-v2"))
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "WorkAttempt authored"
        ):
            verify(data)

    def test_stale_or_missing_failure_feedback_is_rejected(self):
        data = fixture()
        selected = json.loads(data["provenance"]["selected_intents_bytes"])
        selected["intents"]["submit-v2"]["plan"]["instruction"] = "{}"
        encoded = json.dumps(selected, sort_keys=True).encode()
        data["provenance"]["selected_intents_bytes"] = encoded
        data["provenance"]["selected_intents_digest"] = digest(encoded)
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "exact baseline failure feedback"
        ):
            verify(data)

    def test_failed_status_without_failed_output_is_rejected(self):
        data = fixture()
        first = data["report"]["candidate_versions"][0]["checks"][0]
        first["stdout"] = '{"cases":26,"passed":26,"failed":[]}\n'
        data["observation"]["activity"]["checks"][0]["status"] = "failed"
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError,
            "checker stdout differs from retained output bytes",
        ):
            verify(data)

    def test_baseline_candidate_cannot_appear_in_accepted_results(self):
        data = fixture()
        baseline_result = {
            "candidate": {"candidate_id": "result-test", "version": 1},
            "result_id": "bad-result",
        }
        data["observation"]["activity"]["results"].append(baseline_result)
        with self.assertRaisesRegex(
            RECOVERY.RecoveryEvidenceError, "failed baseline Candidate was accepted"
        ):
            verify(data)


if __name__ == "__main__":
    unittest.main()
