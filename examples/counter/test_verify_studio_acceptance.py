from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("verify_studio_acceptance.py")
SPEC = importlib.util.spec_from_file_location("verify_studio_acceptance", MODULE_PATH)
assert SPEC and SPEC.loader
validator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validator)


class StudioAcceptanceVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self) -> None:
        self.directory.cleanup()

    def write_json(self, path: Path, value: object) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        path.chmod(0o600)

    def write_claim_receipt(
        self,
        state_dir: Path,
        room_id: str,
        activation_id: str,
        lease_generation: int,
        operation_id: str,
    ) -> None:
        database = state_dir.parent / "data" / "worldstream.sqlite3"
        database.parent.mkdir(mode=0o700, exist_ok=True)
        result = {
            "operation_id": operation_id,
            "activation_id": activation_id,
            "claim_id": "claim",
            "runner_id": "runner",
            "code": "granted",
            "state": "leased",
            "lease_generation": lease_generation,
            "context_hash": "blake3:" + "a" * 64,
            "context": {
                "activation_id": activation_id,
                "claim_id": "claim",
                "lease_generation": lease_generation,
                "cursor": None,
            },
        }
        connection = validator.sqlite3.connect(database)
        try:
            connection.execute(
                "CREATE TABLE IF NOT EXISTS activation_operation_receipts "
                "(room_id TEXT, operation_id TEXT, operation_kind TEXT, "
                "activation_id TEXT, result_code TEXT, result_bytes BLOB)"
            )
            connection.execute(
                "INSERT INTO activation_operation_receipts VALUES (?, ?, 'claim', ?, 'granted', ?)",
                (room_id, operation_id, activation_id, json.dumps(result).encode()),
            )
            connection.commit()
        finally:
            connection.close()
        database.chmod(0o600)

    @staticmethod
    def completion_witness(
        activation_id: str, *, claim_id: str = "claim", lease_generation: int = 1
    ) -> tuple[str, str, bytes, dict[str, object]]:
        return (
            "operation",
            activation_id,
            b"a" * 32,
            {
                "operation_id": "operation",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "code": "completed",
                "state": "completed",
                "lease_generation": lease_generation,
                "context": None,
            },
        )

    def test_replay_hash_mismatch_cannot_pass(self) -> None:
        studio, console = self.root / "studio.html", self.root / "console.html"
        studio.write_text("<main>Agent Profile prompt label</main>", encoding="utf-8")
        console.write_text(
            "Verified Canonical History at sequence 2 Lineage hash: blake3:" + "a" * 64
            + " Authoritative state hash: blake3:" + "b" * 64,
            encoding="utf-8",
        )
        with self.assertRaisesRegex(validator.VerificationFailure, "replay_dom_evidence_missing"):
            validator.dom_evidence(studio, console, True, {
                "genesis_or_transition_hash": "blake3:" + "c" * 64,
                "authoritative_state_hash": "blake3:" + "b" * 64,
            })

    def test_verify_capture_witness_keeps_safe_completion_shape(self) -> None:
        room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        activation_id = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
        args = SimpleNamespace(
            control_file=self.root / "control.json",
            state_dir=self.root / "studio",
            draft_name="counter4-demo",
            provider_status_url="http://127.0.0.1:19431",
            studio_dom=self.root / "studio.html",
            console_dom=self.root / "console.html",
            url_evidence=self.root / "urls.json",
            mode="capture",
            before_witness=self.root / "before.json",
            witness=self.root / "capture.json",
            channel=[],
        )
        completed = self.completion_witness(activation_id)[3]
        patches = {
            "control_metadata": lambda _path: {"supervisor": "http://127.0.0.1:9410"},
            "setup_for_draft": lambda _state, _draft: {},
            "setup_bindings": lambda _setup: (room_id, "human", "agent", "runner"),
            "exact_template_lineage": lambda *_args: True,
            "creation_and_ready_genesis": lambda *_args: True,
            "loopback_url": lambda value, _code: value,
            "provider_counts": lambda _url: (2, 2),
            "public_room_state": lambda *_args: (2, 2, {
                "genesis_or_transition_hash": "blake3:" + "a" * 64,
                "authoritative_state_hash": "blake3:" + "b" * 64,
            }),
            "completion_receipts": lambda *_args: (("operation", activation_id, b"a" * 32, completed),),
            "bound_launch": lambda *_args: ("c" * 64, {}, {}),
            "recovery_evidence": lambda *_args: True,
            "committed_increment_evidence": lambda *_args: True,
            "dom_evidence": lambda *_args: (True, True),
            "url_evidence": lambda *_args: True,
            "scan_secret_absence": lambda *_args: True,
        }
        with patch.multiple(validator, **patches):
            report = validator.verify(args)
        witness = validator.private_witness(args.witness)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(
            witness["completion_receipts"], [["operation", activation_id, (b"a" * 32).hex()]]
        )

    def test_wrong_template_pack_cannot_pass(self) -> None:
        usage = {"template_id": "template", "revision": "v1", "draft": {"draft_id": "editable"}}
        revision = {"schema": "worldstream/studio-task-template/v1", "template_id": "template", "revision": "v1", "source_draft_id": "source", "pack": {"id": "wrong", "version": "4.0.0"}, "configuration": {"initial_value": 0, "maximum_value": 3}, "seats": [], "readiness": [], "operator_view": True}
        self.write_json(self.root / "task-templates/usages/u.json", usage)
        self.write_json(self.root / "task-templates/revisions/r.json", revision)
        with self.assertRaisesRegex(validator.VerificationFailure, "exact_counter_template_invalid"):
            validator.exact_template_lineage(self.root, "editable", {})

    def test_raw_producer_drafts_preserve_exact_template_lineage(self) -> None:
        pack = {"id": "worldstream.counter", "version": "4.0.0", "digest": validator.COUNTER_V4_DIGEST}
        managed = {
            "seat_id": "counter-2", "role": "counter", "required": True,
            "display_name": "Managed Counter", "principal_id": "agent", "principal_kind": "agent",
            "agent_assignment": "managed", "agent_profile": {"profile_id": "profile", "revision": "v1"},
            "runner_template": {"template_id": "runner", "revision": "v1"},
        }
        human = {"seat_id": "counter-1", "role": "counter", "required": True, "display_name": "Human Counter", "principal_id": "human", "principal_kind": "human"}
        template = {"schema": "worldstream/studio-task-template/v1", "template_id": "template", "revision": "v1", "display_name": "Template", "source_draft_id": "source", "pack": pack, "configuration": {"initial_value": 0, "maximum_value": 3}, "seats": [human, managed], "readiness": [], "operator_view": True}
        instantiated = {"schema": "worldstream/studio-room-draft/v1", "draft_id": "editable", "pack": pack, "configuration": template["configuration"], "seats": template["seats"], "readiness": [], "operator_view": True, "last_valid_step": "readiness"}
        self.write_json(self.root / "task-templates/usages/u.json", {"schema": "worldstream/studio-task-template-usage/v1", "template_id": "template", "revision": "v1", "draft": instantiated})
        self.write_json(self.root / "task-templates/revisions/r.json", template)
        for name in ("source", "editable"):
            draft = {**instantiated, "draft_id": name, "last_valid_step": "review"}
            self.write_json(self.root / f"room-drafts/{name}.json", draft)
        setup = {"seats": [{"principal_kind": "agent", "agent_profile": managed["agent_profile"], "runner": {"managed_assignment": {"template_id": "runner", "template_revision": "v1"}}}]}
        self.assertTrue(validator.exact_template_lineage(self.root, "editable", setup))

    def test_unrelated_ledger_cannot_prove_bound_restart(self) -> None:
        assignment, reference = "01ARZ3NDEKTSV4RRFFQ69G5FB0", "a" * 64
        self.write_json(self.root / "managed-agent-hosts/operations/operation.json", {"schema": "worldstream/managed-agent-host-operation/v1", "assignment_id": assignment, "attempts": 2})
        self.write_json(self.root / "assignment-mcp-activations/other/private/ledger.json", {"lease_generation": 9})
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_lease_recovery_not_retained"):
            validator.recovery_evidence(
                self.root,
                "01ARZ3NDEKTSV4RRFFQ69G5FB2",
                assignment,
                reference,
                self.completion_witness(assignment),
            )

    def test_recovery_evidence_requires_the_bound_accepted_completion_ledger(self) -> None:
        assignment, activation, reference, room_id = (
            "01ARZ3NDEKTSV4RRFFQ69G5FB0",
            "01ARZ3NDEKTSV4RRFFQ69G5FB1",
            "a" * 64,
            "01ARZ3NDEKTSV4RRFFQ69G5FB2",
        )
        state_dir = self.root / "studio"
        state_dir.mkdir(mode=0o700)
        self.write_json(state_dir / "managed-agent-hosts/operations/operation.json", {
            "schema": "worldstream/managed-agent-host-operation/v1",
            "assignment_id": assignment,
            "attempts": 2,
        })
        record = {
            "schema": "worldstream/studio-assignment-mcp-operation@1",
            "phase": "complete",
            "intent": {
                "identity": {"assignment_id": assignment, "operation_id": "operation"},
                "kind": {
                    "kind": "activation_completion",
                    "activation_id": activation,
                    "claim_id": "claim",
                    "acquisition_cursor": 2,
                    "lease_generation": 2,
                    "completion_id": "completion",
                },
            },
            "remote_acceptance": {
                "outcome": "accepted",
                "kind": {
                    "kind": "activation_completion",
                    "activation_id": activation,
                    "completion_id": "completion",
                },
            },
        }
        path = state_dir / f"assignment-mcp-activations/{reference}/operations/completion.json"
        self.write_json(path, record)
        self.write_claim_receipt(
            state_dir, room_id, activation, 2, "claim-generation-two"
        )
        self.assertTrue(validator.recovery_evidence(
            state_dir, room_id, assignment, reference,
            self.completion_witness(activation, lease_generation=2),
        ))
        record["intent"]["identity"]["operation_id"] = "other-operation"
        self.write_json(path, record)
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_lease_recovery_not_retained"):
            validator.recovery_evidence(
                state_dir, room_id, assignment, reference,
                self.completion_witness(activation, lease_generation=2),
            )
        record["intent"]["identity"]["operation_id"] = "operation"
        record["remote_acceptance"]["kind"]["activation_id"] = "wrong"
        self.write_json(path, record)
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_lease_recovery_not_retained"):
            validator.recovery_evidence(
                state_dir, room_id, assignment, reference,
                self.completion_witness(activation, lease_generation=2),
            )

    def test_recovery_evidence_accepts_a_retained_same_generation_lease(self) -> None:
        assignment = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        activation = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
        reference, room_id = "a" * 64, "01ARZ3NDEKTSV4RRFFQ69G5FB2"
        state_dir = self.root / "studio"
        state_dir.mkdir(mode=0o700)
        self.write_json(state_dir / "managed-agent-hosts/operations/operation.json", {
            "schema": "worldstream/managed-agent-host-operation/v1",
            "assignment_id": assignment,
            "attempts": 2,
        })
        completion = {
            "schema": "worldstream/studio-assignment-mcp-operation@1",
            "phase": "complete",
            "intent": {"identity": {"assignment_id": assignment, "operation_id": "operation"}, "kind": {
                "kind": "activation_completion", "activation_id": activation, "claim_id": "claim",
                "acquisition_cursor": 1, "lease_generation": 1, "completion_id": "completion",
            }},
            "remote_acceptance": {"outcome": "accepted", "kind": {
                "kind": "activation_completion", "activation_id": activation, "completion_id": "completion",
            }},
        }
        self.write_json(
            state_dir / f"assignment-mcp-activations/{reference}/operations/completion.json", completion
        )
        self.write_claim_receipt(state_dir, room_id, activation, 1, "claim-generation-one")
        self.assertTrue(validator.recovery_evidence(
            state_dir, room_id, assignment, reference, self.completion_witness(activation)
        ))
        completion["intent"]["kind"]["claim_id"] = "different-claim"
        self.write_json(
            state_dir / f"assignment-mcp-activations/{reference}/operations/completion.json", completion
        )
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_acquisition_not_retained"):
            validator.recovery_evidence(
                state_dir, room_id, assignment, reference, self.completion_witness(
                    activation, claim_id="different-claim"
                )
            )
        completion["intent"]["kind"]["claim_id"] = "claim"
        completion["intent"]["kind"]["lease_generation"] = 2
        self.write_json(
            state_dir / f"assignment-mcp-activations/{reference}/operations/completion.json", completion
        )
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_lease_recovery_not_retained"):
            validator.recovery_evidence(
                state_dir, room_id, assignment, reference, self.completion_witness(activation)
            )

    def test_completed_increment_action_must_bind_the_completed_activation(self) -> None:
        assignment = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        activation = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
        root = self.root / "assignment-mcp-operations" / ("a" * 64)
        request = {
            "assignment_id": assignment, "operation_id": activation,
            "request_id": activation, "action_id": activation,
            "offer_id": "offered-increment", "action_type": "increment", "payload": {},
        }
        record = {
            "intent": {
                "identity": {"assignment_id": assignment, "operation_id": activation},
                "kind": {"kind": "action", "action_id": activation},
                "canonical_request": json.dumps(request, separators=(",", ":")),
            },
            "phase": "complete",
        }
        self.write_json(root / "action.json", record)
        self.assertTrue(validator.committed_increment_evidence(self.root, assignment, "a" * 64, activation))
        request["action_type"] = "private_ack"
        record["intent"]["canonical_request"] = json.dumps(request, separators=(",", ":"))
        self.write_json(root / "action.json", record)
        with self.assertRaisesRegex(validator.VerificationFailure, "increment_action_receipt_missing"):
            validator.committed_increment_evidence(self.root, assignment, "a" * 64, activation)

    def test_retained_invocation_context_values_are_available_only_as_canaries(self) -> None:
        room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        activation_id = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
        state_dir = self.root / "studio"
        state_dir.mkdir(mode=0o700)
        database = self.root / "data" / "worldstream.sqlite3"
        database.parent.mkdir(mode=0o700, exist_ok=True)
        connection = validator.sqlite3.connect(database)
        try:
            connection.execute(
                "CREATE TABLE activation_operation_receipts "
                "(room_id TEXT, activation_id TEXT, operation_kind TEXT, result_code TEXT, context_bytes BLOB)"
            )
            connection.execute(
                "INSERT INTO activation_operation_receipts VALUES (?, ?, 'claim', 'granted', ?)",
                (
                    room_id,
                    activation_id,
                    json.dumps({
                        "projection_bytes": list(
                            b'{"private_context_canary":"nested-secret-leaf"}'
                        )
                    }).encode(),
                ),
            )
            connection.commit()
        finally:
            connection.close()
        database.chmod(0o600)
        values = validator.retained_invocation_context_values(state_dir, room_id, activation_id)
        assert b"nested-secret-leaf" in values
        assert any(value.startswith(b'{"projection_bytes"') for value in values)

    def test_private_context_leaf_filter_excludes_shared_offer_schema_metadata(self) -> None:
        schema_digest = b"blake3:" + b"a" * 64
        context = {
            "activation_id": "activation-private",
            "claim_id": "claim-private",
            "projection_bytes": list(
                b'{"private_context":{"note":"private-content-leaf"}}'
            ),
            "action_offers_bytes": list(
                b'{"offers":[{"payload_schema_digest":"'
                + schema_digest
                + b'"}]}'
            ),
        }
        values = validator.private_context_leaf_values(json.dumps(context).encode("utf-8"))
        self.assertIn(b"private-content-leaf", values)
        self.assertIn(b"activation-private", values)
        self.assertNotIn(schema_digest, values)
        sentinel = self.root / "private-leaf.canary"
        sentinel.write_bytes(b"private-content-leaf")
        sentinel.chmod(0o600)
        console = self.root / "console.html"
        output = self.root / "absence.json"
        console.write_bytes(b"Schema: " + schema_digest)
        allowed = subprocess.run(
            [
                sys.executable,
                str(MODULE_PATH.parents[2] / "scripts/verify-secret-absence.py"),
                "--sentinel-file", str(sentinel),
                "--channel", f"console-dom={console}",
                "--output", str(output),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(allowed.returncode, 0)
        console.write_bytes(b"Schema: " + schema_digest + b" private-content-leaf")
        leaked = subprocess.run(
            [
                sys.executable,
                str(MODULE_PATH.parents[2] / "scripts/verify-secret-absence.py"),
                "--sentinel-file", str(sentinel),
                "--channel", f"console-dom={console}",
                "--output", str(output),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(leaked.returncode, 0)

    def test_large_retained_context_uses_bounded_private_leaf_canaries(self) -> None:
        room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        activation_id = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
        state_dir = self.root / "studio"
        state_dir.mkdir(mode=0o700)
        database = self.root / "data" / "worldstream.sqlite3"
        database.parent.mkdir(mode=0o700, exist_ok=True)
        context = json.dumps({
            "activation_id": "activation-private-identity",
            "claim_id": "claim-private-identity",
            "projection_bytes": list(
                b'{"private_context":{"note":"private-content-leaf"}}'
            ),
            "padding": "x" * 1_024,
        }).encode("utf-8")
        connection = validator.sqlite3.connect(database)
        try:
            connection.execute(
                "CREATE TABLE activation_operation_receipts "
                "(room_id TEXT, activation_id TEXT, operation_kind TEXT, result_code TEXT, context_bytes BLOB)"
            )
            connection.execute(
                "INSERT INTO activation_operation_receipts VALUES (?, ?, 'claim', 'granted', ?)",
                (room_id, activation_id, context),
            )
            connection.commit()
        finally:
            connection.close()
        database.chmod(0o600)
        values = validator.retained_invocation_context_values(state_dir, room_id, activation_id)
        self.assertNotIn(context, values)
        self.assertIn(b"private-content-leaf", values)
        self.assertIn(b"activation-private-identity", values)

    def test_url_evidence_requires_both_browser_channels_and_rejects_room_reference(self) -> None:
        evidence = self.root / "urls.json"
        self.write_json(evidence, {
            "schema": "worldstream/counter-browser-url-evidence/v1",
            "studio_current_url": "http://127.0.0.1:5174/",
            "console_current_url": "http://127.0.0.1:5173/",
            "studio_request_urls": ["http://127.0.0.1:5174/"],
            "console_request_urls": ["http://127.0.0.1:5173/?room_id=private"],
        })
        with self.assertRaisesRegex(validator.VerificationFailure, "browser_url_disclosed_protected_material"):
            validator.url_evidence(evidence)
        self.write_json(evidence, {
            "schema": "worldstream/counter-browser-url-evidence/v1",
            "studio_current_url": "http://127.0.0.1:5174/",
            "console_current_url": "http://127.0.0.1:5173/",
            "studio_request_urls": ["http://127.0.0.1:5174/"],
            "console_request_urls": [],
        })
        with self.assertRaisesRegex(validator.VerificationFailure, "browser_url_evidence_incomplete"):
            validator.url_evidence(evidence)

    def test_extra_room_inventory_cannot_pass(self) -> None:
        room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
        original = validator.public_get
        validator.public_get = lambda _base, _path: {
            "schema": "worldstream/studio-room-inventory/v1",
            "rooms": [{"room_id": room_id}, {"room_id": "01ARZ3NDEKTSV4RRFFQ69G5FB1"}],
        }
        try:
            with self.assertRaisesRegex(validator.VerificationFailure, "room_inventory_not_exactly_one"):
                validator.public_room_state("http://127.0.0.1:9410", room_id)
        finally:
            validator.public_get = original

    def test_opaque_reference_in_console_cannot_pass(self) -> None:
        studio, console = self.root / "studio.html", self.root / "console.html"
        studio.write_text("<main>Agent Profile prompt label</main>", encoding="utf-8")
        console.write_text("secret_reference: " + "a" * 64, encoding="utf-8")
        with self.assertRaisesRegex(validator.VerificationFailure, "browser_dom_disclosed_protected_material"):
            validator.dom_evidence(studio, console, False, {
                "genesis_or_transition_hash": "blake3:" + "a" * 64,
                "authoritative_state_hash": "blake3:" + "b" * 64,
            })

    def test_group_readable_protected_record_cannot_pass(self) -> None:
        record = self.root / "protected.json"
        self.write_json(record, {"safe": True})
        record.chmod(0o644)
        with self.assertRaisesRegex(validator.VerificationFailure, "owner_only"):
            validator.safe_json(record, "owner_only")

    def test_secret_scan_is_rerunnable_without_retaining_canaries(self) -> None:
        sentinel = self.root / "sentinel"
        sentinel.write_bytes(b"private")
        studio, console = self.root / "studio.html", self.root / "console.html"
        studio.write_text("studio", encoding="utf-8")
        console.write_text("console", encoding="utf-8")
        args = SimpleNamespace(
            witness=self.root / "witness.json", mode="capture", state_dir=self.root,
            studio_dom=studio, console_dom=console, url_evidence=self.root / "urls.json", channel=[],
        )
        self.write_json(args.url_evidence, {
            "schema": "worldstream/counter-browser-url-evidence/v1",
            "studio_current_url": "http://127.0.0.1:5174/",
            "console_current_url": "http://127.0.0.1:5173/",
            "studio_request_urls": ["http://127.0.0.1:5174/"],
            "console_request_urls": ["http://127.0.0.1:5173/"],
        })
        launch = {"authority_reference": "a" * 64}
        with patch.object(validator, "retained_secret_canaries", return_value={"member": sentinel}), patch.object(
            validator.subprocess, "run", return_value=SimpleNamespace(returncode=0)
        ):
            self.assertTrue(validator.scan_secret_absence(args, {}, launch, launch, "a" * 64, "room", "activation"))
            self.assertTrue(validator.scan_secret_absence(args, {}, launch, launch, "a" * 64, "room", "activation"))
        self.assertEqual(list(self.root.glob(".imo89-capture-*")), [])


if __name__ == "__main__":
    unittest.main()


def test_private_context_leaf_sentinel_detects_a_leaf_only_dom_leak(tmp_path: Path) -> None:
    sentinel = tmp_path / "private-leaf"
    sentinel.write_bytes(b"nested-secret-leaf")
    sentinel.chmod(0o600)
    console = tmp_path / "console.html"
    console.write_text("visible nested-secret-leaf only", encoding="utf-8")
    output = tmp_path / "result.json"
    result = subprocess.run(
        [
            sys.executable,
            str(MODULE_PATH.parents[2] / "scripts/verify-secret-absence.py"),
            "--sentinel-file", str(sentinel),
            "--channel", f"console-dom={console}",
            "--output", str(output),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0
