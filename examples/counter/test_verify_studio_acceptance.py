from __future__ import annotations

import importlib.util
import json
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

    def test_wrong_template_pack_cannot_pass(self) -> None:
        usage = {"template_id": "template", "revision": "v1", "draft": {"draft_id": "editable"}}
        revision = {"schema": "worldstream/studio-task-template/v1", "template_id": "template", "revision": "v1", "source_draft_id": "source", "pack": {"id": "wrong", "version": "3.0.0"}, "configuration": {"initial_value": 0, "maximum_value": 3}, "seats": [], "readiness": [], "operator_view": True}
        self.write_json(self.root / "task-templates/usages/u.json", usage)
        self.write_json(self.root / "task-templates/revisions/r.json", revision)
        with self.assertRaisesRegex(validator.VerificationFailure, "exact_counter_template_invalid"):
            validator.exact_template_lineage(self.root, "editable", {})

    def test_raw_producer_drafts_preserve_exact_template_lineage(self) -> None:
        pack = {"id": "worldstream.counter", "version": "3.0.0", "digest": validator.COUNTER_V3_DIGEST}
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
        self.write_json(self.root / f"managed-agent-hosts/{assignment}.json", {"schema": "worldstream/managed-agent-host-operation/v1", "assignment_id": assignment, "attempts": 2})
        self.write_json(self.root / "assignment-mcp-activations/other/private/ledger.json", {"lease_generation": 9})
        with self.assertRaisesRegex(validator.VerificationFailure, "activation_lease_recovery_not_retained"):
            validator.recovery_evidence(self.root, assignment, reference, ("operation", assignment, b"a" * 32))

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
            studio_dom=studio, console_dom=console, channel=[],
        )
        launch = {"authority_reference": "a" * 64}
        with patch.object(validator, "retained_secret_canaries", return_value={"member": sentinel}), patch.object(
            validator.subprocess, "run", return_value=SimpleNamespace(returncode=0)
        ):
            self.assertTrue(validator.scan_secret_absence(args, {}, launch, launch))
            self.assertTrue(validator.scan_secret_absence(args, {}, launch, launch))
        self.assertEqual(list(self.root.glob(".imo89-capture-*")), [])


if __name__ == "__main__":
    unittest.main()
