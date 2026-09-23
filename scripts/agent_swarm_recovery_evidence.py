"""Offline verification for one model-led Agent Swarm delivery recovery.

This module verifies retained Room, checker, selected-intent, and native-turn
bytes to establish Candidate authorship and an independent fresh review.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any


class RecoveryEvidenceError(ValueError):
    """The supplied evidence does not support a recovery claim."""


def _fail(message: str) -> None:
    raise RecoveryEvidenceError(message)


def _digest(data: bytes, expected: str) -> bool:
    algorithm, separator, value = expected.partition(":")
    if not separator or len(value) != 64:
        return False
    if algorithm == "sha256":
        actual = hashlib.sha256(data).hexdigest()
    elif algorithm == "blake3":
        try:
            from blake3 import blake3
        except (
            ImportError
        ) as error:  # pragma: no cover - exercised in runtime packaging
            raise RecoveryEvidenceError(
                "BLAKE3 support is required to verify retained artifacts"
            ) from error
        actual = blake3(data).hexdigest()
    else:
        return False
    return actual == value


def _json_bytes(data: bytes, label: str) -> dict[str, Any]:
    try:
        value = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RecoveryEvidenceError(f"{label} is not valid JSON") from error
    if not isinstance(value, dict):
        _fail(f"{label} must be a JSON object")
    return value


def _candidate_key(candidate: dict[str, Any]) -> str:
    try:
        candidate_id = candidate["candidate_id"]
        version = candidate["version"]
    except (KeyError, TypeError):
        _fail("candidate reference is incomplete")
    if (
        not isinstance(candidate_id, str)
        or not candidate_id
        or not isinstance(version, int)
    ):
        _fail("candidate reference is invalid")
    return f"{candidate_id}:{version}"


def _same_candidate(left: Any, right: Any) -> bool:
    return (
        isinstance(left, dict)
        and isinstance(right, dict)
        and (left.get("candidate_id"), left.get("version"))
        == (right.get("candidate_id"), right.get("version"))
    )


def _walk(value: Any):
    if isinstance(value, dict):
        for key, child in value.items():
            yield key, child
            yield from _walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk(child)


def _instruction_objects(value: Any, depth: int = 0) -> list[Any]:
    """Decode bounded JSON strings embedded in worker instructions."""
    if depth > 8:
        return []
    values = [value]
    if isinstance(value, str):
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return values
        return values + _instruction_objects(parsed, depth + 1)
    if isinstance(value, dict):
        for child in value.values():
            values.extend(_instruction_objects(child, depth + 1))
    elif isinstance(value, list):
        for child in value:
            values.extend(_instruction_objects(child, depth + 1))
    return values


def _check_feedback(instruction: Any, failed_check: dict[str, Any]) -> bool:
    expected_check = failed_check["room"]
    expected_stdout = failed_check["stdout"]
    for root in _instruction_objects(instruction):
        for key, feedback in _walk(root):
            if key != "check_feedback" or not isinstance(feedback, dict):
                continue
            checks = feedback.get("checks")
            if not isinstance(checks, list):
                continue
            if any(
                row.get("candidate_version")
                == expected_check.get("candidate", {}).get("version")
                and row.get("check_id") == expected_check.get("check_id")
                and row.get("status") == "failed"
                and row.get("stdout") == expected_stdout
                for row in checks
                if isinstance(row, dict)
            ):
                return True
    return False


def _candidate_action_contract(instruction: str) -> dict[str, Any] | None:
    for root in _instruction_objects(instruction):
        if isinstance(root, dict) and root.get("task") == "integrate":
            if "next_version" in root:
                return root
            contract = root.get("action_contract")
            if isinstance(contract, dict):
                return contract
            if isinstance(contract, str):
                try:
                    parsed = json.loads(contract)
                except json.JSONDecodeError:
                    continue
                if isinstance(parsed, dict):
                    return parsed
    return None


def _native_record(
    invocation_id: str,
    selected: dict[str, Any],
    native_stdout_by_invocation: dict[str, dict[str, Any]],
) -> tuple[dict[str, Any], dict[str, Any]]:
    intent = selected.get("intents", {}).get(invocation_id)
    if not isinstance(intent, dict):
        _fail(f"selected intent {invocation_id} is missing")
    evidence = intent.get("evidence")
    if (
        not isinstance(evidence, dict)
        or evidence.get("success") is not True
        or evidence.get("exit_code") != 0
    ):
        _fail("native invocation did not complete successfully")
    output = evidence.get("output")
    plan = intent.get("plan")
    if not isinstance(output, dict) or not isinstance(plan, dict):
        _fail("native invocation is missing its output or selected plan")
    session_id = output.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        _fail("native invocation omits its provider session identity")
    for plan_key, output_key in (
        ("model", "reported_model"),
        ("effort", "reported_effort"),
    ):
        expected = plan.get(plan_key) or plan.get(f"requested_{plan_key}")
        actual = output.get(output_key)
        if expected and actual != expected:
            _fail(f"native invocation {plan_key} differs from the selected plan")
    native = native_stdout_by_invocation.get(invocation_id)
    if not isinstance(native, dict) or not isinstance(native.get("bytes"), bytes):
        _fail(f"native stdout bytes are missing for {invocation_id}")
    expected_digest = evidence.get("stdout")
    if isinstance(expected_digest, dict):
        expected_digest = expected_digest.get("digest")
    if not isinstance(expected_digest, str) or native.get("digest") != expected_digest:
        _fail("native stdout reference does not match its retained artifact")
    if not _digest(native["bytes"], expected_digest):
        _fail("native stdout bytes do not match the retained digest")
    if native.get("byte_length") != len(native["bytes"]):
        _fail("native stdout byte length differs from its manifest")
    envelope = _json_bytes(native["bytes"], "native turn stdout")
    if envelope.get("schema") != "worldstream/codex-app-server-turn@1":
        _fail("native stdout has an unsupported turn schema")
    turn = envelope.get("completion", {}).get("turn", {})
    if turn.get("status") != "completed" or turn.get("error") is not None:
        _fail("native model turn did not complete successfully")
    configuration = envelope.get("configuration", {})
    if configuration.get("provider_session_id") != session_id:
        _fail("native stdout session differs from the retained invocation session")
    if output.get("reported_model") != configuration.get("model"):
        _fail("native stdout model differs from the retained invocation model")
    if output.get("reported_effort") != configuration.get("effort"):
        _fail("native stdout effort differs from the retained invocation effort")
    turn_items = turn.get("items", [])
    if isinstance(turn_items, list) and turn_items:
        final_messages = [
            item.get("text")
            for item in turn_items
            if isinstance(item, dict) and isinstance(item.get("text"), str)
        ]
        if len(final_messages) > 1:
            final_messages = final_messages[-1:]
    else:
        final_messages = [
            message.get("text")
            for message in envelope.get("messages", [])
            if message.get("type") == "agentMessage"
            and message.get("phase") == "final_answer"
        ]
    if len(final_messages) != 1 or final_messages[0] != output.get("text"):
        _fail("native final response differs from the retained invocation output")
    try:
        parsed_final = json.loads(final_messages[0])
    except json.JSONDecodeError as error:
        raise RecoveryEvidenceError(
            "native final response is not a JSON action proposal"
        ) from error
    if parsed_final != intent.get("proposal"):
        _fail(
            "native final response does not exactly match the retained selected proposal"
        )
    if output.get("thread_id") and turn.get("threadId") != output.get("thread_id"):
        _fail("native turn thread differs from the retained invocation thread")
    output["session_id"] = session_id
    output["native_final_text"] = final_messages[0]
    if not isinstance(parsed_final, dict):
        _fail("native final response proposal must be an object")
    output["native_final_json"] = parsed_final
    return intent, output


def _normalize_declaration(
    declaration: dict[str, Any], report: dict[str, Any]
) -> dict[str, Any]:
    """Validate the persisted pre-run declaration and derive fixed policy."""
    if declaration.get("schema") != "worldstream/agent-swarm-recovery-declaration@1":
        _fail("pre-run recovery declaration has an unsupported schema")
    baseline_sha = declaration.get("baseline_source_sha256", "")
    if not isinstance(baseline_sha, str) or not baseline_sha.startswith("sha256:"):
        _fail("pre-run declaration does not pin the baseline SHA-256")
    try:
        baseline_hex = baseline_sha.split(":", 1)[1]
        if len(baseline_hex) != 64:
            raise ValueError
        bytes.fromhex(baseline_hex)
    except ValueError:
        _fail("pre-run baseline SHA-256 is malformed")
    criteria = declaration.get("acceptance_criteria")
    criteria_text = declaration.get("criteria_bytes")
    if not isinstance(criteria, list) or not isinstance(criteria_text, str):
        _fail("pre-run declaration omits exact acceptance criteria")
    expected_criteria = json.dumps(criteria, ensure_ascii=False, separators=(",", ":"))
    if criteria_text != expected_criteria:
        _fail("pre-run criteria bytes do not encode the declared acceptance criteria")
    criteria_bytes = criteria_text.encode("utf-8")
    criteria_digest = declaration.get("criteria_digest")
    if not _digest(criteria_bytes, criteria_digest or ""):
        _fail("pre-run criteria bytes do not match their frozen digest")
    if declaration.get("fixed_check_count") != 26:
        _fail("pre-run declaration does not pin the 26-case fixed checker")
    checker = declaration.get("checker")
    preflight = declaration.get("preflight")
    if not isinstance(checker, dict) or not isinstance(preflight, dict):
        _fail("pre-run declaration omits checker identity or preflight")
    if preflight.get("checker") != checker:
        _fail("baseline preflight used a different checker command")
    if preflight.get("checker_sha256") != declaration.get("fixed_checker_sha256"):
        _fail("baseline preflight checker digest differs from frozen declaration")
    if report.get("fixed_checker_sha256") != declaration.get("fixed_checker_sha256"):
        _fail("report fixed checker digest differs from frozen pre-run declaration")
    if report.get("baseline_checker") != preflight:
        _fail("report baseline preflight differs from the retained declaration")
    expected_failure = {
        "cases": 26,
        "passed": 25,
        "failed": ["quote_in_unquoted_field"],
    }
    if (
        preflight.get("exit_code") != 1
        or preflight.get("output") != expected_failure
        or preflight.get("stderr") != ""
    ):
        _fail("baseline preflight does not show the one predeclared real failing case")
    check_ids = declaration.get("required_check_ids")
    if not isinstance(check_ids, list) or check_ids != [
        f"criterion-{index}" for index in range(1, len(criteria) + 1)
    ]:
        _fail("pre-run declaration has noncanonical fixed check IDs")
    criteria_revision = declaration.get("criteria_revision")
    if not isinstance(criteria_revision, int) or criteria_revision <= 0:
        _fail("pre-run declaration omits the fixed criteria revision")
    return {
        "baseline_source_sha256": baseline_hex,
        "baseline_source_byte_length": declaration.get("baseline_source_byte_length"),
        "required_check_ids": check_ids,
        "checker": checker,
        "checker_executable_sha256": declaration.get("checker_executable_sha256"),
        "criteria_digest": criteria_digest,
        "criteria_revision": criteria_revision,
        "criteria_bytes": criteria_bytes,
        "input_digests": declaration.get("evidence_input_bindings", []),
        "expected_case_count": 26,
        "baseline_failed_cases": expected_failure["failed"],
        "resource_basis": [
            {
                "resource_id": declaration.get("policy", {})
                .get("delivery", {})
                .get("resource_id"),
                "version": declaration.get("policy", {})
                .get("delivery", {})
                .get("expected_resource_version"),
            }
        ],
    }


def _verify_model_provenance(
    report: dict[str, Any],
    observation: dict[str, Any],
    provenance: dict[str, Any],
    later: dict[str, Any],
    review_refs: list[dict[str, Any]],
) -> str:
    selected_bytes = provenance.get("selected_intents_bytes")
    selected_digest = provenance.get("selected_intents_digest")
    if (
        not isinstance(selected_bytes, bytes)
        or not isinstance(selected_digest, str)
        or not _digest(selected_bytes, selected_digest)
    ):
        _fail("retained selected-intent journal bytes are missing or invalid")
    selected = _json_bytes(selected_bytes, "selected-intent journal")
    if selected.get("schema") != "worldstream/agent-swarm-recovery-provenance@1":
        _fail("selected-intent journal has an unsupported schema")
    native = provenance.get("native_stdout_by_invocation", {})
    if not isinstance(native, dict):
        _fail("native stdout evidence map is malformed")
    authored = None
    candidate_bytes = None
    for invocation_id, intent in selected.get("intents", {}).items():
        plan = intent.get("plan", {})
        target = plan.get("semantic_target", {})
        proposal = intent.get("proposal", {})
        artifact = proposal.get("artifact", {}) if isinstance(proposal, dict) else {}
        inline = artifact.get("inline_text") if isinstance(artifact, dict) else None
        if not isinstance(inline, str) or not inline:
            continue
        if inline.encode("utf-8") != provenance.get("repaired_candidate_bytes"):
            continue
        action = intent.get("action", {})
        if (
            not isinstance(action, dict)
            or action.get("action_type") != "submit_candidate"
        ):
            continue
        action_payload = action.get("payload", {})
        if not isinstance(action_payload, dict) or any(
            action_payload.get(key) != later.get(candidate_key)
            for key, candidate_key in (
                ("candidate_id", "candidate_id"),
                ("work_id", "work_id"),
                ("expected_work_revision", "work_revision"),
                ("contribution_refs", "contribution_refs"),
                ("resource_basis", "resource_basis"),
                ("artifact", "artifact"),
            )
        ):
            continue
        submission = intent.get("submission", {})
        if (
            submission.get("status") not in {"received", "accepted"}
            or submission.get("value", {}).get("status") != "accepted"
        ):
            continue
        if intent.get("member_id") != later.get("author_member_id"):
            continue
        if target.get("kind") != "work_attempt" or target.get("work_id") != later.get(
            "work_id"
        ):
            continue
        native_intent, output = _native_record(invocation_id, selected, native)
        instruction = native_intent.get("plan", {}).get("instruction")
        if not isinstance(instruction, str):
            _fail("repaired Candidate WorkAttempt omits its exact worker instruction")
        failed_checks = [
            item
            for row in report.get("candidate_versions", [])
            if row.get("candidate", {}).get("version") == 1
            for item in row.get("checks", [])
            if item.get("room_check", {}).get("status") == "failed"
        ]
        if not failed_checks or not _check_feedback(
            instruction,
            {
                "room": failed_checks[0]["room_check"],
                "stdout": failed_checks[0]["stdout"],
            },
        ):
            _fail(
                "repaired Candidate worker did not receive the exact baseline failure feedback"
            )
        contract = _candidate_action_contract(instruction)
        if not isinstance(contract, dict) or any(
            (
                contract.get(key)
                if key != "target.work_id"
                else contract.get("target", {}).get("work_id")
            )
            != expected
            for key, expected in (
                ("candidate_id", later.get("candidate_id")),
                ("next_version", later.get("version")),
                ("target.work_id", later.get("work_id")),
                ("contribution_refs", later.get("contribution_refs")),
                ("resource_basis", later.get("resource_basis")),
            )
        ):
            _fail(
                "selected worker instruction does not authorize the repaired Candidate version and basis"
            )
        output_value = output["native_final_json"]
        native_inline = next(
            (value for key, value in _walk(output_value) if key == "inline_text"), None
        )
        if native_inline != inline:
            _fail(
                "native final response does not exactly contain the submitted Candidate text"
            )
        if output_value.get("payload", {}).get("candidate_id") != later.get(
            "candidate_id"
        ):
            _fail("native proposal does not name the repaired Candidate identity")
        authored = (invocation_id, native_intent, output)
        candidate_bytes = inline.encode("utf-8")
        break
    if authored is None or candidate_bytes != provenance.get(
        "repaired_candidate_bytes"
    ):
        _fail(
            "no retained successful selected WorkAttempt authored the repaired Candidate bytes"
        )

    failed_checks = [
        item
        for row in report.get("candidate_versions", [])
        if row.get("candidate", {}).get("version") == 1
        for item in row.get("checks", [])
        if item.get("room_check", {}).get("status") == "failed"
    ]
    if not failed_checks or not _check_feedback(
        instruction,
        {
            "room": failed_checks[0]["room_check"],
            "stdout": failed_checks[0]["stdout"],
        },
    ):
        _fail(
            "repaired Candidate worker did not receive the exact baseline failure feedback"
        )

    delivery_actions_bytes = provenance.get("delivery_actions_bytes")
    delivery_actions_digest = provenance.get("delivery_actions_digest")
    if (
        not isinstance(delivery_actions_bytes, bytes)
        or not isinstance(delivery_actions_digest, str)
        or not _digest(delivery_actions_bytes, delivery_actions_digest)
    ):
        _fail("retained delivery action journal bytes are missing or invalid")
    delivery_actions = _json_bytes(delivery_actions_bytes, "delivery action journal")
    if (
        delivery_actions.get("schema")
        != "worldstream/agent-swarm-recovery-delivery-actions@1"
    ):
        _fail("delivery action journal has an unsupported schema")
    failed_record_seq = None
    for operation in delivery_actions.get("operations", {}).values():
        for item in operation.get("actions", []):
            action = item.get("action", {})
            payload = action.get("payload", {})
            receipt = item.get("receipt", {}) or {}
            if (
                action.get("action_type") == "record_check"
                and payload.get("candidate", {}).get("version") == 1
                and payload.get("status") == "failed"
                and receipt.get("status") == "accepted"
                and isinstance(receipt.get("room_seq"), int)
            ):
                failed_record_seq = receipt["room_seq"]
    submission = authored[1].get("submission", {})
    submission_value = submission.get("value", {})
    candidate_submit_seq = (
        submission_value.get("room_seq") if isinstance(submission_value, dict) else None
    )
    if (
        not isinstance(failed_record_seq, int)
        or not isinstance(candidate_submit_seq, int)
        or failed_record_seq >= candidate_submit_seq
    ):
        _fail(
            "repaired Candidate submission does not follow the accepted failed Check receipt"
        )

    authored_session = authored[2].get("session_id")
    for ref in review_refs:
        room_review = next(
            (
                r
                for r in observation.get("activity", {}).get("reviews", [])
                if r.get("review_id") == ref.get("id")
                and r.get("revision") == ref.get("revision")
            ),
            None,
        )
        if (
            room_review is None
            or not _same_candidate(room_review.get("candidate"), later)
            or room_review.get("verdict") != "passed"
        ):
            continue
        for invocation_id, intent in selected.get("intents", {}).items():
            plan = intent.get("plan", {})
            target = plan.get("semantic_target", {})
            session = plan.get("session", {})
            if target.get("kind") != "candidate_review":
                continue
            if target.get("candidate_id") != later.get("candidate_id") or target.get(
                "candidate_version"
            ) != later.get("version"):
                continue
            if target.get("review_id") != ref.get("id"):
                continue
            if plan.get("member_id", intent.get("member_id")) != room_review.get(
                "reviewer_member_id"
            ):
                continue
            if session.get("mode") != "fresh":
                continue
            action = intent.get("action", {})
            action_payload = (
                action.get("payload", {}) if isinstance(action, dict) else {}
            )
            if (
                not isinstance(action, dict)
                or action.get("action_type") != "record_review"
                or action_payload.get("review_id") != ref.get("id")
                or action_payload.get("candidate")
                != {
                    "candidate_id": later.get("candidate_id"),
                    "version": later.get("version"),
                }
                or action_payload.get("verdict") != "passed"
            ):
                continue
            review_intent, output = _native_record(invocation_id, selected, native)
            if output.get("session_id") == authored_session:
                continue
            if (
                output["native_final_json"].get("payload", {}).get("verdict")
                != "passed"
            ):
                continue
            if output["native_final_json"].get("payload", {}).get(
                "review_id"
            ) != ref.get("id"):
                continue
            if output["native_final_json"].get("payload", {}).get("candidate") != {
                "candidate_id": later.get("candidate_id"),
                "version": later.get("version"),
            }:
                continue
            if target.get("criteria_revision") != room_review.get("criteria_revision"):
                continue
            if output.get("session_id"):
                review_submission = review_intent.get("submission", {}).get("value", {})
                if (
                    not isinstance(review_submission, dict)
                    or review_submission.get("status") != "accepted"
                ):
                    continue
                return ref["id"]
    _fail("no fresh independent model review invocation supports the accepted Result")


def _check_documents(
    report: dict[str, Any],
    delivery_journal: dict[str, Any],
    check_evidence_by_digest: dict[str, bytes],
    output_bytes_by_digest: dict[str, bytes],
    criteria_bytes_by_digest: dict[str, bytes],
    expected_case_count: int,
    baseline_failed_cases: list[str],
) -> dict[str, dict[str, Any]]:
    checks_by_candidate: dict[str, dict[str, Any]] = {}
    versions = report.get("candidate_versions")
    if not isinstance(versions, list):
        _fail("report candidate_versions is missing")
    for version_row in versions:
        candidate = version_row.get("candidate", {})
        key = _candidate_key(candidate)
        row_checks = version_row.get("checks")
        if not isinstance(row_checks, list):
            _fail(f"checks are missing for Candidate {key}")
        checks_by_candidate[key] = {}
        for row in row_checks:
            room_check = row.get("room_check")
            if not isinstance(room_check, dict):
                _fail(f"Room check is missing for Candidate {key}")
            if not _same_candidate(room_check.get("candidate"), candidate):
                _fail("check is attributed to a different Candidate")
            refs = room_check.get("evidence_refs")
            if not isinstance(refs, list) or len(refs) != 1:
                _fail("check must contain exactly one evidence reference")
            ref = refs[0].get("artifact", {})
            evidence_digest = ref.get("digest")
            evidence_bytes = check_evidence_by_digest.get(evidence_digest)
            if not isinstance(evidence_bytes, bytes) or not _digest(
                evidence_bytes, evidence_digest
            ):
                _fail(
                    "check evidence bytes do not match the exact Room artifact digest"
                )
            evidence = _json_bytes(evidence_bytes, "check evidence")
            if evidence.get("check_id") != room_check.get("check_id"):
                _fail("check evidence ID does not match the Room check")
            pack_candidate_artifact = refs[0].get("candidate_artifact")
            if pack_candidate_artifact != candidate.get("artifact"):
                _fail("check reference is bound to different Candidate artifact bytes")
            expected_local_digest = (
                delivery_journal.get("versions", {})
                .get(str(candidate.get("version")), {})
                .get("sealed", {})
                .get("candidate_digest")
            )
            if evidence.get("binding", {}).get("candidate") != expected_local_digest:
                _fail(
                    "check evidence is bound to a different sealed Candidate revision"
                )
            criteria_ref = evidence.get("binding", {}).get("criteria", {})
            criteria_digest = criteria_ref.get("digest")
            criteria_bytes = criteria_bytes_by_digest.get(criteria_digest)
            if (
                not isinstance(criteria_bytes, bytes)
                or not _digest(criteria_bytes, criteria_digest)
                or criteria_ref.get("byte_length") != len(criteria_bytes)
            ):
                _fail("exact acceptance-criteria bytes are missing or invalid")
            stdout_ref = evidence.get("stdout", {}).get("digest")
            stderr_ref = evidence.get("stderr", {}).get("digest")
            stdout_bytes = output_bytes_by_digest.get(stdout_ref)
            stderr_bytes = output_bytes_by_digest.get(stderr_ref)
            if not isinstance(stdout_bytes, bytes) or not _digest(
                stdout_bytes, stdout_ref
            ):
                _fail("checker stdout bytes do not match the evidence digest")
            if not isinstance(stderr_bytes, bytes) or not _digest(
                stderr_bytes, stderr_ref
            ):
                _fail("checker stderr bytes do not match the evidence digest")
            if len(stdout_bytes) != evidence.get("stdout", {}).get(
                "byte_length"
            ) or len(stderr_bytes) != evidence.get("stderr", {}).get("byte_length"):
                _fail("checker output byte length differs from exact evidence")
            if room_check.get("status") == "passed":
                if evidence.get("outcome") != {"outcome": "passed"}:
                    _fail(
                        "Room marks a check passed but exact checker evidence does not"
                    )
                try:
                    result = json.loads(stdout_bytes.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    raise RecoveryEvidenceError(
                        "passing checker stdout is not a structured result"
                    ) from error
                if (
                    not isinstance(result, dict)
                    or result.get("cases") != expected_case_count
                    or result.get("passed") != result.get("cases")
                    or result.get("failed") != []
                ):
                    _fail("passing checker stdout does not show every case passed")
            elif room_check.get("status") == "failed":
                outcome = evidence.get("outcome")
                if (
                    not isinstance(outcome, dict)
                    or outcome.get("outcome") != "failed"
                    or not isinstance(outcome.get("exit_code"), int)
                    or outcome.get("exit_code") == 0
                ):
                    _fail(
                        "Room marks a check failed without an actual failed checker outcome"
                    )
                try:
                    result = json.loads(stdout_bytes.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError) as error:
                    raise RecoveryEvidenceError(
                        "failed checker stdout is not a structured result"
                    ) from error
                if (
                    not isinstance(result, dict)
                    or result.get("cases") != expected_case_count
                    or not isinstance(result.get("passed"), int)
                    or result["passed"] >= result["cases"]
                    or not isinstance(result.get("failed"), list)
                    or not result["failed"]
                ):
                    _fail("failed checker stdout does not show a real failing case")
                if (
                    candidate.get("version") == 1
                    and result["failed"] != baseline_failed_cases
                ):
                    _fail(
                        "baseline failure differs from the predeclared defective case set"
                    )
            else:
                _fail("check has an unsupported Room status")

            if row.get("stdout") != stdout_bytes.decode("utf-8", errors="replace"):
                _fail("reported checker stdout differs from retained output bytes")
            if row.get("stderr") != stderr_bytes.decode("utf-8", errors="replace"):
                _fail("reported checker stderr differs from retained output bytes")
            check_id = room_check.get("check_id")
            if check_id in checks_by_candidate[key]:
                _fail("duplicate check ID for one Candidate")
            checks_by_candidate[key][check_id] = {
                "room": room_check,
                "evidence": evidence,
                "criteria_bytes": criteria_bytes,
                "stdout": stdout_bytes.decode("utf-8", errors="replace"),
                "stderr": stderr_bytes.decode("utf-8", errors="replace"),
            }
    return checks_by_candidate


def verify_recovery(
    report: dict[str, Any],
    observation: dict[str, Any],
    delivery_journal: dict[str, Any],
    declaration: dict[str, Any],
    candidate_bytes_by_ref: dict[str, bytes],
    check_evidence_by_digest: dict[str, bytes],
    output_bytes_by_digest: dict[str, bytes],
    criteria_bytes_by_digest: dict[str, bytes],
    provenance: dict[str, Any],
) -> dict[str, Any]:
    """Verify a failed-baseline -> revised-Candidate -> accepted recovery.

    `provenance` supplies retained selected intents, native stdout, and delivery
    actions. This function verifies their bytes and joins them to Room state;
    it does not authenticate provider signatures.
    """
    if report.get("status") != "passed":
        _fail("autonomous delivery report did not pass")
    if (
        not isinstance(report.get("candidate_versions"), list)
        or len(report["candidate_versions"]) < 2
    ):
        _fail("recovery requires a failed baseline and a later Candidate")
    if report.get("delivery_plans_staged_by_evaluation_driver") != 0:
        _fail("evaluation driver staged a post-start delivery plan")
    if report.get("evaluation_driver_post_setup_actions") != 0:
        _fail("evaluation driver submitted a post-setup Action")
    if (
        report.get("baseline_first_instructed") is not True
        or declaration.get("baseline_first_instructed") is not True
    ):
        _fail(
            "pre-run declaration and report must establish baseline-first instruction"
        )
    declared = _normalize_declaration(declaration, report)
    if (
        report.get("baseline_source_sha256")
        != "sha256:" + declared["baseline_source_sha256"]
    ):
        _fail("report baseline digest differs from retained pre-run declaration")

    activity = observation.get("activity")
    if not isinstance(activity, dict):
        _fail("final observation has no Activity state")
    if activity.get("criteria_revision") != declared["criteria_revision"]:
        _fail("final Room criteria revision differs from the frozen declaration")
    report_versions = report.get("candidate_versions")
    room_candidates = activity.get("candidates")
    if not isinstance(report_versions, list) or len(report_versions) < 2:
        _fail("recovery requires a failed baseline and a later Candidate")
    if not isinstance(room_candidates, list):
        _fail("final observation Candidates are missing")
    ordered = sorted(
        report_versions, key=lambda row: row.get("candidate", {}).get("version", 0)
    )
    first = ordered[0].get("candidate", {})
    if first.get("version") != 1:
        _fail("first Candidate is not version 1")
    first_key = _candidate_key(first)
    baseline = candidate_bytes_by_ref.get(first_key)
    if not isinstance(baseline, bytes):
        _fail("exact baseline Candidate bytes are missing")
    expected_sha = declared["baseline_source_sha256"]
    if hashlib.sha256(baseline).hexdigest() != expected_sha:
        _fail("first Candidate bytes do not equal the declared defective baseline")
    if len(baseline) != declared["baseline_source_byte_length"]:
        _fail("first Candidate byte length differs from pre-run declaration")
    room_first = next((c for c in room_candidates if _same_candidate(c, first)), None)
    if room_first is None or room_first.get("artifact", {}).get("digest") != first.get(
        "artifact", {}
    ).get("digest"):
        _fail("first Candidate does not exactly match final Room state")
    if not _digest(baseline, first.get("artifact", {}).get("digest", "")):
        _fail("baseline Candidate bytes do not match the Room artifact digest")

    expected_case_count = declared["expected_case_count"]
    baseline_failed_cases = declared["baseline_failed_cases"]
    checks = _check_documents(
        report,
        delivery_journal,
        check_evidence_by_digest,
        output_bytes_by_digest,
        criteria_bytes_by_digest,
        expected_case_count,
        baseline_failed_cases,
    )
    first_checks = checks.get(first_key, {})
    expected_check_ids = declared["required_check_ids"]
    if not expected_check_ids:
        _fail("declaration must name unchanged fixed check IDs")
    if set(first_checks) != set(expected_check_ids):
        _fail("baseline does not have exactly the declared fixed checks")
    failed = [
        first_checks[check_id]
        for check_id in expected_check_ids
        if first_checks[check_id]["room"].get("status") == "failed"
    ]
    if not failed:
        _fail("baseline Candidate has no actual failed checker")
    baseline_checker = declared["checker"]
    criteria_digest = declared["criteria_digest"]
    criteria_revision = activity.get("criteria_revision")
    for key, version_checks in checks.items():
        for check_id in expected_check_ids:
            if check_id not in version_checks:
                _fail(f"fixed check {check_id} missing for Candidate {key}")
            evidence = version_checks[check_id]["evidence"]
            checker_identity = evidence.get("checker", {})
            if (
                checker_identity.get("executable_path")
                != baseline_checker.get("program")
                or checker_identity.get("arguments")
                != baseline_checker.get("arguments")
                or checker_identity.get("environment")
                != baseline_checker.get("environment")
            ):
                _fail("checker identity changed between Candidate versions")
            if delivery_journal.get("executables", {}).get(
                checker_identity.get("executable_path")
            ) != checker_identity.get("executable", {}).get("digest"):
                _fail(
                    "checker executable digest does not match the retained delivery journal"
                )
            if (
                evidence.get("binding", {}).get("criteria", {}).get("digest")
                != criteria_digest
            ):
                _fail("acceptance criteria artifact changed between Candidate versions")
            if version_checks[check_id]["criteria_bytes"] != declared["criteria_bytes"]:
                _fail("acceptance criteria content changed between Candidate versions")
            input_binding = evidence.get("binding", {}).get("inputs", [])
            expected_inputs = declared["input_digests"]
            observed_inputs = [
                {
                    "name": item.get("name"),
                    "digest": item.get("artifact", {}).get("digest"),
                }
                for item in input_binding
            ]
            if observed_inputs != expected_inputs:
                _fail("check input binding changed between Candidate versions")
            room_check = version_checks[check_id]["room"]
            if room_check.get("criteria_revision") != criteria_revision:
                _fail("acceptance criteria revision changed between Candidate versions")
            index = int(check_id.rsplit("-", 1)[1]) - 1
            expected_criterion = declaration["acceptance_criteria"][index]
            if room_check.get("criterion") != expected_criterion:
                _fail(
                    "fixed acceptance criterion text changed between Candidate versions"
                )
            if room_check.get("resource_basis") != declared["resource_basis"]:
                _fail("check resource basis changed between Candidate versions")
    observed_room_checks = activity.get("checks", [])
    for version_checks in checks.values():
        for item in version_checks.values():
            room_check = item["room"]
            if room_check not in observed_room_checks:
                _fail("report contains a check absent from final Room history")

    later = ordered[-1].get("candidate", {})
    later_key = _candidate_key(later)
    if later.get("version", 0) <= 1:
        _fail("repair Candidate is not a later version")
    repaired = candidate_bytes_by_ref.get(later_key)
    if not isinstance(repaired, bytes):
        _fail("exact repaired Candidate bytes are missing")
    if repaired == baseline:
        _fail("later Candidate bytes are unchanged from the defective baseline")
    if not _digest(repaired, later.get("artifact", {}).get("digest", "")):
        _fail("repaired Candidate bytes do not match the Room artifact digest")
    room_later = next((c for c in room_candidates if _same_candidate(c, later)), None)
    if room_later is None or room_later != later:
        _fail("repaired Candidate is absent from final Room state")
    contribution_refs = later.get("contribution_refs", [])
    if not contribution_refs:
        _fail("repaired Candidate does not identify its exact Contributions")
    for contribution_ref in contribution_refs:
        if not any(
            item.get("contribution_id") == contribution_ref.get("contribution_id")
            and item.get("version") == contribution_ref.get("version")
            for item in activity.get("contributions", [])
        ):
            _fail("repaired Candidate references a missing Contribution version")
    later_checks = checks.get(later_key, {})
    if set(later_checks) != set(expected_check_ids) or any(
        later_checks[check_id]["room"].get("status") != "passed"
        for check_id in expected_check_ids
    ):
        _fail("repaired Candidate lacks fresh passing fixed checks")

    results = activity.get("results")
    accepted = report.get("accepted_results")
    if not isinstance(results, list) or not isinstance(accepted, list) or not accepted:
        _fail("accepted Result evidence is missing")
    if any(_same_candidate(result.get("candidate"), first) for result in results):
        _fail("failed baseline Candidate was accepted")
    result = accepted[-1]
    if not _same_candidate(result.get("candidate"), later):
        _fail("accepted Result names a Candidate other than the repaired version")
    if result.get("criteria_revision") != criteria_revision:
        _fail("accepted Result uses a different acceptance criteria revision")
    if result.get("resource_basis") != declared["resource_basis"]:
        _fail("accepted Result uses a different resource basis")
    room_result = next(
        (r for r in results if r.get("result_id") == result.get("result_id")), None
    )
    if room_result != result:
        _fail("reported accepted Result does not match final Room state")
    expected_refs = [
        {"id": check_id, "revision": later_checks[check_id]["room"].get("revision")}
        for check_id in expected_check_ids
    ]
    if result.get("check_refs") != expected_refs:
        _fail(
            "accepted Result does not reference the repaired Candidate's current checks"
        )
    if (
        report.get("final_source_sha256")
        != "sha256:" + hashlib.sha256(repaired).hexdigest()
    ):
        _fail("delivered source digest does not match the accepted repaired Candidate")
    review_refs = result.get("review_refs")
    room_reviews = activity.get("reviews", [])
    if not isinstance(review_refs, list) or not review_refs:
        _fail("accepted Result has no review references")
    independent_room_review = False
    author_ids = {later.get("author_member_id")}
    for contribution_ref in later.get("contribution_refs", []):
        contribution = next(
            (
                item
                for item in activity.get("contributions", [])
                if item.get("contribution_id")
                == contribution_ref.get("contribution_id")
                and item.get("version") == contribution_ref.get("version")
            ),
            None,
        )
        if contribution is not None and contribution.get("author_member_id"):
            author_ids.add(contribution["author_member_id"])
    for ref in review_refs:
        review = next(
            (
                r
                for r in room_reviews
                if r.get("review_id") == ref.get("id")
                and r.get("revision") == ref.get("revision")
            ),
            None,
        )
        if (
            review is not None
            and _same_candidate(review.get("candidate"), later)
            and review.get("verdict") == "passed"
            and review.get("reviewer_member_id") not in author_ids
        ):
            independent_room_review = True
    if not independent_room_review:
        _fail("accepted Result lacks a fresh independent passing review")
    verified_provenance = dict(provenance)
    verified_provenance["repaired_candidate_bytes"] = repaired
    independent_review_id = _verify_model_provenance(
        report, observation, verified_provenance, later, review_refs
    )

    return {
        "status": "passed",
        "baseline_candidate": first,
        "baseline_sha256": "sha256:" + expected_sha,
        "failed_check_ids": [
            check_id
            for check_id in expected_check_ids
            if first_checks[check_id]["room"].get("status") == "failed"
        ],
        "repaired_candidate": later,
        "repaired_sha256": hashlib.sha256(repaired).hexdigest(),
        "passing_check_ids": expected_check_ids,
        "accepted_result_id": result.get("result_id"),
        "independent_review_id": independent_review_id,
        "evaluation_driver_post_setup_actions": 0,
        "delivery_plans_staged_by_evaluation_driver": 0,
        "model_authorship_provenance": "verified_from_retained_intents_and_native_output",
    }


def authorship_sessions(provenance: dict[str, Any]) -> set[str]:
    """Return session IDs already bound to Candidate authorship evidence."""
    authorship = provenance.get("revised_candidate_authorship", {})
    sessions = (
        authorship.get("author_session_ids", []) if isinstance(authorship, dict) else []
    )
    return {value for value in sessions if isinstance(value, str) and value}
