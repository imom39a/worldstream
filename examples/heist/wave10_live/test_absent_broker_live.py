from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

import tomllib
from worldstream_sdk import ProtocolError

# The repository-wide SDK gate invokes the console entry point, whose import
# path does not include this sibling example directory. Keep the test runnable
# both from the repository root and directly from this directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from run_absent_broker_live import (
    CREATE_IDEMPOTENCY_KEY,
    MEMBER_IDEMPOTENCY_KEYS,
    PACK_DIGEST,
    PACK_ID,
    PRINCIPAL_IDS,
    RUNNER_CAPABILITY_IDEMPOTENCY_KEY,
    RUNNER_IDEMPOTENCY_KEY,
    RUNNER_PRINCIPAL_IDEMPOTENCY_KEY,
    TIMER_PHASE,
    BlockedStory,
    _activation_receipt,
    _assert_heist_projection,
    _owner_only_regular_file,
    _plan_from_published_claims,
    _retry_retained_runner_request,
    _verify_replay_hash_parity,
    run_live,
)


class AbsentBrokerLiveHarnessTests(unittest.IsolatedAsyncioTestCase):
    def test_published_participant_claims_select_each_frozen_fixture(self) -> None:
        self.assertEqual(
            _plan_from_published_claims("route_canal", "entry_window_late"),
            {
                "route": "canal",
                "entry_window": "late",
                "required_tool": "disguise",
                "extraction": "van",
            },
        )
        self.assertEqual(
            _plan_from_published_claims("route_service", "entry_window_early"),
            {
                "route": "service",
                "entry_window": "early",
                "required_tool": "thermal_key",
                "extraction": "boat",
            },
        )
        self.assertEqual(
            _plan_from_published_claims("route_roof", "entry_window_middle"),
            {
                "route": "roof",
                "entry_window": "middle",
                "required_tool": "jammer",
                "extraction": "motorbike",
            },
        )
        with self.assertRaises(BlockedStory) as invalid:
            _plan_from_published_claims("route_canal", "entry_window_early")
        self.assertEqual(
            invalid.exception.reason_code, "published_clue_fixture_pair_invalid"
        )

    async def test_retained_runner_retry_reuses_identity_across_transient_busy(
        self,
    ) -> None:
        request = {"message_id": "same", "body": {"claim_id": "same"}}

        class Runner:
            def __init__(self) -> None:
                self.requests: list[dict] = []

            async def retry(self, candidate: dict, *, timeout: float) -> dict:
                self.requests.append(candidate)
                if len(self.requests) == 1:
                    raise ProtocolError("room_busy", "busy", True)
                return {"code": "granted"}

        runner = Runner()
        result = await _retry_retained_runner_request(runner, request, timeout=1.0)
        self.assertEqual(result, {"code": "granted"})
        self.assertEqual(runner.requests, [request, request])
        self.assertIs(runner.requests[0], runner.requests[1])

    async def test_missing_live_inputs_are_fail_closed(self) -> None:
        class Args:
            base_url = None
            operator_bearer = None

        result = await run_live(Args())
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(
            result["reason_code"], "base_url_and_operator_bearer_are_required"
        )
        self.assertFalse(result["fabricated_offer"])
        self.assertEqual(result["secrets"], "not_emitted")

    def test_timer_request_has_no_payload_surface(self) -> None:
        self.assertEqual(TIMER_PHASE, "01ARZ3NDEKTSV4RRFFQ69G5FH0")
        self.assertEqual(
            set({"timer_id": TIMER_PHASE, "generation": 1}), {"timer_id", "generation"}
        )
        self.assertNotIn("payload", {"timer_id", "generation"})

    def test_projection_rejects_final_reveal_fields_before_completion(self) -> None:
        response = {"projection": {"activity": {"phase": "briefing"}}}
        _assert_heist_projection(response, "navigator")
        response["projection"]["activity"]["fixture"] = {"required_tool": "secret"}
        with self.assertRaises(BlockedStory):
            _assert_heist_projection(response, "navigator")

    def test_activation_receipt_redacts_context(self) -> None:
        result = _activation_receipt(
            {
                "code": "granted",
                "state": "leased",
                "lease_generation": 1,
                "context_hash": "blake3:hash",
                "context": {"projection": {"private_clues": ["secret"]}},
            }
        )
        self.assertTrue(result["context_present"])
        self.assertTrue(result["context_hash_present"])
        self.assertNotIn("context", result)

    def test_pack_identity_is_exact(self) -> None:
        self.assertEqual(PACK_ID, "worldstream.agent-heist")
        manifest = tomllib.loads(
            (Path(__file__).resolve().parents[3] / "compatibility.toml").read_text()
        )
        selectable = [
            row
            for row in manifest["pack_executors"]
            if row["pack_id"] == PACK_ID
            and row["explanatory_version"] == "0.1.0"
            and row["selectable_for_new_rooms"] is True
        ]
        self.assertEqual(len(selectable), 1)
        self.assertEqual(PACK_DIGEST, selectable[0]["revision_digest"])
        self.assertEqual(len(CREATE_IDEMPOTENCY_KEY), 26)
        self.assertTrue(CREATE_IDEMPOTENCY_KEY.startswith("01"))
        self.assertEqual(len(set(MEMBER_IDEMPOTENCY_KEYS)), 3)
        self.assertEqual(len(set(PRINCIPAL_IDS)), 3)
        self.assertTrue(set(MEMBER_IDEMPOTENCY_KEYS).isdisjoint(PRINCIPAL_IDS))
        self.assertEqual(
            len(
                {
                    RUNNER_PRINCIPAL_IDEMPOTENCY_KEY,
                    RUNNER_IDEMPOTENCY_KEY,
                    RUNNER_CAPABILITY_IDEMPOTENCY_KEY,
                }
            ),
            3,
        )

    def test_replay_hash_parity_requires_all_public_head_fields(self) -> None:
        head = {
            "room_id": "room",
            "room_seq": 7,
            "pack_digest": "blake3:pack",
            "genesis_or_transition_hash": "blake3:transition",
            "core_state_hash": "blake3:core",
            "activity_state_hash": "blake3:activity",
            "authoritative_state_hash": "blake3:aggregate",
        }
        result = _verify_replay_hash_parity(
            {"room_head": head}, {"room_head": dict(head)}
        )
        self.assertTrue(result["verified"])
        self.assertEqual(
            set(result["fields"]),
            {
                "core",
                "pack",
                "activity",
                "aggregate_authoritative",
                "transition",
                "room_id",
                "room_seq",
            },
        )

        missing = dict(head)
        del missing["authoritative_state_hash"]
        with self.assertRaises(BlockedStory) as error:
            _verify_replay_hash_parity({"room_head": head}, {"room_head": missing})
        self.assertEqual(
            error.exception.reason_code, "replay_hash_parity_fields_unexposed"
        )

    def test_replay_hash_parity_rejects_mismatch_without_derivation(self) -> None:
        head = {
            "room_id": "room",
            "room_seq": 7,
            "pack_digest": "blake3:pack",
            "genesis_or_transition_hash": "blake3:transition",
            "core_state_hash": "blake3:core",
            "activity_state_hash": "blake3:activity",
            "authoritative_state_hash": "blake3:aggregate",
        }
        replay_head = dict(head)
        replay_head["core_state_hash"] = "blake3:other"
        with self.assertRaises(BlockedStory) as error:
            _verify_replay_hash_parity({"room_head": head}, {"room_head": replay_head})
        self.assertEqual(error.exception.reason_code, "replay_hash_parity_mismatch")

    def test_postgresql_dsn_file_must_be_owner_only_and_not_a_symlink(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-wave10-dsn-") as name:
            root = Path(name)
            dsn = root / "runtime.dsn"
            dsn.write_text(
                "credential material is intentionally opaque", encoding="utf-8"
            )
            dsn.chmod(0o600)
            self.assertEqual(_owner_only_regular_file(dsn), dsn.absolute())

            dsn.chmod(0o640)
            with self.assertRaises(BlockedStory) as permissions:
                _owner_only_regular_file(dsn)
            self.assertEqual(
                permissions.exception.reason_code,
                "postgresql_dsn_file_not_owner_only",
            )

            dsn.chmod(0o600)
            link = root / "runtime-link.dsn"
            link.symlink_to(dsn)
            with self.assertRaises(BlockedStory) as symlink:
                _owner_only_regular_file(link)
            self.assertEqual(
                symlink.exception.reason_code,
                "postgresql_dsn_file_not_regular",
            )


if __name__ == "__main__":
    unittest.main()
