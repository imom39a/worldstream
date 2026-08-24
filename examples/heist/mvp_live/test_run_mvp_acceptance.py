from __future__ import annotations

import importlib.util
import json
import pathlib
import unittest

MODULE_PATH = pathlib.Path(__file__).with_name("run_mvp_acceptance.py")
SPEC = importlib.util.spec_from_file_location("run_mvp_acceptance", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MVP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MVP)


def completed_report() -> dict[str, object]:
    return {
        "schema": "worldstream/agent-heist-mvp-acceptance/v1",
        "status": "completed",
        "components": {
            "daemon": "real_process_http_websocket",
            "supervisor": "real_process_http",
            "assignment_mcp": "real_process_stdio",
            "studio": "production_http_contract",
            "participant_console": "production_origin_http_contract",
        },
        "task": {
            "draft_id": "agent-heist-mvp",
            "reviewed": True,
            "room_ids": ["01ARZ3NDEKTSV4RRFFQ69G5FAV"],
            "seats": [
                {"seat_id": "navigator-1", "principal_kind": "human"},
                {
                    "seat_id": "insider-1",
                    "principal_kind": "agent",
                    "agent_assignment": "external",
                },
            ],
            "lobby_observed": True,
            "readiness_observed": True,
            "explicit_launch": True,
            "authoritative_room_count_before": 0,
            "authoritative_room_count_after": 1,
        },
        "human": {
            "handoff_redeemed": True,
            "session_usable": True,
            "actions_committed": 4,
        },
        "agent": {
            "assignment_count": 1,
            "task_count": 1,
            "observation_acknowledged": True,
            "listed_offers_only": True,
            "actions_committed": 4,
            "activation_leased": True,
            "helper_restarted": True,
            "stable_launch_reference_reused": True,
            "duplicate_actions": 0,
            "duplicate_completions": 0,
        },
        "outcome": {
            "phase": "complete",
            "meaningful": True,
            "committed_room_seq": 17,
            "authoritative_state_hash": "blake3:" + "a" * 64,
            "activity_state_hash": "blake3:" + "b" * 64,
            "transition_hash": "blake3:" + "d" * 64,
            "outcome_digest": "blake3:" + "c" * 64,
        },
        "replay": {
            "verified": True,
            "at_room_seq": 17,
            "authoritative_state_hash": "blake3:" + "a" * 64,
            "activity_state_hash": "blake3:" + "b" * 64,
            "transition_hash": "blake3:" + "d" * 64,
            "outcome_digest": "blake3:" + "c" * 64,
            "transition_ids_unique": True,
        },
        "privacy": {
            "checked_surfaces": [
                "studio_http",
                "participant_console_http",
                "assignment_mcp_stdio",
                "process_logs",
            ],
            "credential_disclosure": False,
            "private_activation_disclosure": False,
        },
    }


class MvpAcceptanceContractTests(unittest.TestCase):
    def test_assignment_launch_is_private_launcher_control(self) -> None:
        class Launcher:
            def request(self, method, path, body=None, **kwargs):
                self.call = (method, path, body, kwargs)
                return 201, {"launch_reference": "a" * 64}, {}

        launcher = Launcher()
        response = MVP.LiveGate._issue_assignment_launch(
            launcher, "01ARZ3NDEKTSV4RRFFQ69G5FAV"
        )
        self.assertEqual(response["launch_reference"], "a" * 64)
        self.assertEqual(launcher.call[0], "POST")
        self.assertEqual(launcher.call[3]["expected"], (200, 201))

    def test_task_setup_reconciliation_polls_and_retries_only_documented_states(
        self,
    ) -> None:
        fetched = iter([{"state": "ready"}])
        pauses: list[float] = []
        result = MVP.reconcile_task_setup(
            {"state": "provisioning"},
            lambda: next(fetched),
            lambda: self.fail("provisioning must poll, not retry"),
            maximum_attempts=2,
            pause=pauses.append,
        )
        self.assertEqual(result["state"], "ready")
        self.assertEqual(pauses, [0.2])

        retries = iter([{"state": "ready"}])
        result = MVP.reconcile_task_setup(
            {
                "state": "needs_attention",
                "attention": {
                    "code": "daemon_result_ambiguous",
                    "retryable": True,
                },
            },
            lambda: self.fail("ambiguous setup must retry the stable operation"),
            lambda: next(retries),
            maximum_attempts=2,
            pause=lambda _: None,
        )
        self.assertEqual(result["state"], "ready")

        with self.assertRaisesRegex(
            MVP.AcceptanceFailure, "task_setup_terminal_failure"
        ):
            MVP.reconcile_task_setup(
                {"state": "needs_attention", "attention": {"retryable": False}},
                dict,
                dict,
                maximum_attempts=1,
                pause=lambda _: None,
            )

    def test_browser_adapter_uses_only_configured_repo_python(self) -> None:
        configured = MVP.sys.executable
        self.assertEqual(
            MVP._browser_python({"WORLDSTREAM_PYTHON": configured}),
            pathlib.Path(configured),
        )
        with self.assertRaisesRegex(MVP.AcceptanceFailure, "browser_python_missing"):
            MVP._browser_python({"WORLDSTREAM_PYTHON": "/definitely/not/python"})
        self.assertEqual(
            MVP._browser_failure(b"blocked:browser_navigation_failed\n").args,
            ("production_console_browser_navigation_failed",),
        )
        self.assertEqual(
            MVP._browser_failure(b"private or unbounded output").args,
            ("production_console_browser_failed",),
        )

    def test_completed_report_requires_exact_story_and_replay_parity(self) -> None:
        MVP.validate_completed_report(completed_report())

        extra_room = completed_report()
        extra_room["task"]["room_ids"].append("01ARZ3NDEKTSV4RRFFQ69G5FAW")
        with self.assertRaisesRegex(MVP.AcceptanceFailure, "exactly_one_room"):
            MVP.validate_completed_report(extra_room)

        wrong_replay = completed_report()
        wrong_replay["replay"]["authoritative_state_hash"] = "blake3:" + "b" * 64
        with self.assertRaisesRegex(MVP.AcceptanceFailure, "replay_hash_mismatch"):
            MVP.validate_completed_report(wrong_replay)

        wrong_activity = completed_report()
        wrong_activity["replay"]["activity_state_hash"] = "blake3:" + "f" * 64
        with self.assertRaisesRegex(MVP.AcceptanceFailure, "replay_activity_mismatch"):
            MVP.validate_completed_report(wrong_activity)

        wrong_transition = completed_report()
        wrong_transition["replay"]["transition_hash"] = "blake3:" + "e" * 64
        with self.assertRaisesRegex(
            MVP.AcceptanceFailure, "replay_transition_mismatch"
        ):
            MVP.validate_completed_report(wrong_transition)

    def test_completed_report_requires_one_human_and_one_external_agent(self) -> None:
        wrong = completed_report()
        wrong["task"]["seats"][1]["agent_assignment"] = "managed"
        with self.assertRaisesRegex(MVP.AcceptanceFailure, "exact_seat_shape"):
            MVP.validate_completed_report(wrong)

    def test_privacy_scan_rejects_bearers_launch_refs_and_private_activation(
        self,
    ) -> None:
        safe = json.dumps(completed_report(), sort_keys=True).encode()
        MVP.assert_public_evidence_safe([("report", safe)], private_values=[])

        for leaked in (
            b"wsb1:" + b"a" * 64,
            b"wst1:" + b"b" * 64,
            b"wsl1:" + b"c" * 64,
            b'"activation_context":{"private":"value"}',
        ):
            with (
                self.subTest(leaked=leaked[:12]),
                self.assertRaisesRegex(
                    MVP.AcceptanceFailure, "credential_or_private_data"
                ),
            ):
                MVP.assert_public_evidence_safe(
                    [("surface", leaked)], private_values=[]
                )

        with self.assertRaisesRegex(
            MVP.AcceptanceFailure, "credential_or_private_data"
        ):
            MVP.assert_public_evidence_safe(
                [("surface", b'{"state":"ready","opaque":"canary"}')],
                private_values=[b"canary"],
            )

    def test_handoff_evidence_redacts_fragment_credential(self) -> None:
        response = {
            "schema": "worldstream/participant-console-handoff/v1",
            "console_url": "http://127.0.0.1:4173/#handoff=wsh1:" + "a" * 64,
        }
        retained = MVP.retained_handoff_evidence(response)
        self.assertEqual(
            retained["console_url"],
            "http://127.0.0.1:4173/#handoff=[REDACTED]",
        )
        self.assertIn("wsh1:", response["console_url"])
        MVP.assert_public_evidence_safe(
            [("studio_http", json.dumps(retained).encode())],
            private_values=[response["console_url"].split("#handoff=", 1)[1].encode()],
        )

    def test_finish_heist_fires_commitment_reminder_before_phase_deadline(self) -> None:
        runner = object.__new__(MVP.LiveGate)
        events: list[tuple[str, int]] = []
        runner._agent_act = lambda *_args, **_kwargs: {}
        runner._human_act = lambda *_args, **_kwargs: {}
        runner._fire_timer = lambda _daemon, _headers, _room, timer, generation: (
            events.append((timer, generation)) or {}
        )
        story = {
            "plan_id": MVP.ACTION_IDS["plan"],
            "transition_ids": [],
            "human_actions": 0,
            "agent_actions": 0,
        }
        runner._finish_heist(None, {}, None, "room", story)
        self.assertLess(
            events.index((MVP.TIMER_REMINDER, 1)),
            events.index((MVP.TIMER_PHASE, 3)),
        )

    def test_json_rpc_client_emits_only_bounded_standard_requests(self) -> None:
        request = MVP.mcp_request(
            4,
            "worldstream.submit_action",
            {
                "operation_id": "01ARZ3NDEKTSV4RRFFQ69G5FC6",
                "offer_id": "7:0:blake3:" + "d" * 64,
                "precondition": {"room_seq": 7, "head_hash": "blake3:" + "e" * 64},
                "payload": {"clue_id": "entry_window"},
            },
        )
        parsed = json.loads(request)
        self.assertEqual(parsed["method"], "tools/call")
        self.assertEqual(parsed["params"]["name"], "worldstream.submit_action")
        self.assertNotIn("room_id", request)
        self.assertNotIn("member_id", request)
        self.assertLessEqual(len(request.encode()), MVP.MAX_MCP_LINE_BYTES)

        with self.assertRaisesRegex(MVP.AcceptanceFailure, "invalid_mcp_tool"):
            MVP.mcp_request(5, "worldstream.admin", {})


if __name__ == "__main__":
    unittest.main()
