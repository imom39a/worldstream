from __future__ import annotations

import json
import stat
import sys
import tempfile
import unittest
from pathlib import Path

import blake3

sys.path.insert(0, str(Path(__file__).parent))

from install_managed_counter_fixture import install_fixture


class InstallManagedCounterFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory(
            prefix="worldstream-managed-fixture-test-"
        )
        self.root = Path(self.directory.name)
        self.root.chmod(0o700)
        self.source = self.root / "worldstream-managed-agent-host"
        self.source.write_bytes(b"#!/bin/sh\nexit 0\n")
        self.source.chmod(0o700)
        self.install_root = self.root / "fixture"
        self.manifests = self.root / "runner-templates"
        self.state_dir = self.root / "studio"
        self.credential_manifests = self.root / "model-provider-credentials"
        self.credential = self.root / "model-token"
        self.credential.write_bytes(b"fixture-model-token-not-for-logs")
        self.credential.chmod(0o600)

    def tearDown(self) -> None:
        self.directory.cleanup()

    def test_installs_an_owner_only_immutable_host_copy_and_exact_counter_v3_manifest(
        self,
    ) -> None:
        install_fixture(self.source, self.install_root, self.manifests, 19431)

        digest = blake3.blake3(self.source.read_bytes()).hexdigest()
        installed_host = self.install_root / "managed-host" / digest / self.source.name
        self.assertEqual(installed_host.read_bytes(), self.source.read_bytes())
        self.assertEqual(stat.S_IMODE(installed_host.stat().st_mode), 0o700)

        manifest = json.loads(
            (self.manifests / "counter-managed-reference-v1.json").read_text("utf-8")
        )
        self.assertEqual(manifest["executable"]["path"], str(installed_host.resolve()))
        self.assertEqual(manifest["executable"]["blake3"], digest)
        self.assertEqual(
            manifest["compatibility"],
            [{"activity_pack_id": "worldstream.counter", "exact_revisions": ["3.0.0"]}],
        )
        self.assertEqual(
            stat.S_IMODE(
                (self.manifests / "counter-managed-reference-v1.json").stat().st_mode
            ),
            0o600,
        )

        provider = json.loads(
            (self.install_root / "managed-counter-provider.json").read_text("utf-8")
        )
        self.assertEqual(provider["bind_address"], "127.0.0.1")
        self.assertEqual(provider["port"], 19431)
        self.assertEqual(
            stat.S_IMODE(
                (self.install_root / "managed-counter-provider.json").stat().st_mode
            ),
            0o600,
        )

    def test_repeat_install_reuses_exact_copy_and_rejects_a_changed_immutable_manifest(
        self,
    ) -> None:
        install_fixture(self.source, self.install_root, self.manifests, 19431)
        install_fixture(self.source, self.install_root, self.manifests, 19431)

        manifest_path = self.manifests / "counter-managed-reference-v1.json"
        manifest = json.loads(manifest_path.read_text("utf-8"))
        manifest["display_name"] = "changed"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        manifest_path.chmod(0o600)

        with self.assertRaisesRegex(ValueError, "immutable"):
            install_fixture(self.source, self.install_root, self.manifests, 19431)

    def test_refuses_a_non_regular_source_or_changed_saved_provider_configuration(
        self,
    ) -> None:
        self.source.unlink()
        self.source.symlink_to("missing-host")
        with self.assertRaisesRegex(ValueError, "regular"):
            install_fixture(self.source, self.install_root, self.manifests, 19431)

        self.source.unlink()
        self.source.write_bytes(b"#!/bin/sh\nexit 0\n")
        self.source.chmod(0o755)
        install_fixture(self.source, self.install_root, self.manifests, 19431)
        with self.assertRaisesRegex(ValueError, "provider configuration"):
            install_fixture(self.source, self.install_root, self.manifests, 19432)

    def test_imports_the_file_only_model_token_behind_a_named_kind_bound_reference(
        self,
    ) -> None:
        install_fixture(
            self.source,
            self.install_root,
            self.manifests,
            19431,
            credential_file=self.credential,
            supervisor_state_dir=self.state_dir,
            model_provider_credentials_dir=self.credential_manifests,
        )

    def test_ignores_an_unrelated_valid_model_credential_in_the_shared_vault(
        self,
    ) -> None:
        vault = self.state_dir / "secrets"
        self.state_dir.mkdir(mode=0o700)
        vault.mkdir(mode=0o700)
        other = vault / ("model-provider-" + "a" * 64 + ".secret")
        other.write_bytes(b"unrelated-model-token")
        other.chmod(0o600)

        install_fixture(
            self.source,
            self.install_root,
            self.manifests,
            19431,
            credential_file=self.credential,
            supervisor_state_dir=self.state_dir,
            model_provider_credentials_dir=self.credential_manifests,
        )

        reference = json.loads(
            (self.credential_manifests / "local-openai.json").read_text("utf-8")
        )["secret"]["reference"]
        self.assertNotEqual(reference, "a" * 64)
        self.assertEqual(other.read_bytes(), b"unrelated-model-token")

        self.assertEqual(stat.S_IMODE(self.state_dir.stat().st_mode), 0o700)

        manifest_path = self.credential_manifests / "local-openai.json"
        manifest = json.loads(manifest_path.read_text("utf-8"))
        reference = manifest["secret"]["reference"]
        self.assertEqual(manifest["schema"], "worldstream/model-provider-credential/v1")
        self.assertEqual(manifest["credential_id"], "local-openai")
        self.assertEqual(manifest["provider"], "open_ai_compatible")
        self.assertEqual(manifest["secret"]["kind"], "model_provider")
        self.assertRegex(reference, r"^[0-9a-f]{64}$")
        self.assertNotIn(
            self.credential.read_text("utf-8"), manifest_path.read_text("utf-8")
        )

        stored = self.state_dir / "secrets" / f"model-provider-{reference}.secret"
        self.assertEqual(stored.read_bytes(), self.credential.read_bytes())
        self.assertEqual(stat.S_IMODE(stored.stat().st_mode), 0o600)
        install_fixture(
            self.source,
            self.install_root,
            self.manifests,
            19431,
            credential_file=self.credential,
            supervisor_state_dir=self.state_dir,
            model_provider_credentials_dir=self.credential_manifests,
        )


if __name__ == "__main__":
    unittest.main()
