from __future__ import annotations

import json
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from deterministic_loopback_provider import create_provider_server


class DeterministicLoopbackProviderTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory(
            prefix="worldstream-provider-test-"
        )
        self.credential = b"fixture-model-token-not-for-logs"
        self.credential_path = Path(self.directory.name) / "model-token"
        self.credential_path.write_bytes(self.credential)
        self.credential_path.chmod(0o600)
        self.server = create_provider_server("127.0.0.1", 0, self.credential_path)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self) -> None:
        self.server.shutdown()
        self.thread.join(timeout=2)
        self.server.server_close()
        self.directory.cleanup()

    def request(
        self, body: object, credential: bytes | None = None
    ) -> dict[str, object]:
        port = self.server.server_address[1]
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/v1/chat/completions",
            data=json.dumps(body, separators=(",", ":")).encode("utf-8"),
            method="POST",
            headers={
                "Authorization": "Bearer "
                + (credential or self.credential).decode("ascii"),
                "Content-Type": "application/json",
            },
        )
        with urllib.request.urlopen(request, timeout=2) as response:
            self.assertEqual(response.status, 200)
            return json.loads(response.read())

    def test_http_provider_selects_the_exact_increment_offer_without_echoing_the_token(
        self,
    ) -> None:
        response = self.request(
            {
                "model": "counter-fixture",
                "content": {
                    "instruction": "Select one offered action.",
                    "observation": {
                        "value": 0,
                        "private_context_canary": "not-for-output",
                    },
                    "offers": [
                        {"offer_id": "private-offer", "action_type": "private_ack"},
                        {"offer_id": "increment-offer", "action_type": "increment"},
                    ],
                },
            }
        )

        choice = response["choices"][0]["message"]
        self.assertEqual(choice["role"], "assistant")
        self.assertEqual(
            json.loads(choice["content"]),
            {"offer_id": "increment-offer", "payload": {}},
        )
        self.assertNotIn(self.credential.decode("ascii"), json.dumps(response))

    def test_http_provider_accepts_the_managed_host_json_message_content(self) -> None:
        response = self.request(
            {
                "model": "counter-fixture",
                "response_format": {"type": "json_object"},
                "messages": [
                    {
                        "role": "user",
                        "content": json.dumps(
                            {
                                "instruction": "Select one offered action.",
                                "observation": {"value": 1},
                                "offers": {
                                    "schema": "worldstream/assignment-current-action-offers/v1",
                                    "offers": [
                                        {
                                            "offer_id": "exact-increment-offer",
                                            "action_type": "increment",
                                        }
                                    ],
                                },
                            }
                        ),
                    }
                ],
            }
        )

        self.assertEqual(
            json.loads(response["choices"][0]["message"]["content"]),
            {"offer_id": "exact-increment-offer", "payload": {}},
        )

        with urllib.request.urlopen(
            f"http://127.0.0.1:{self.server.server_address[1]}/fixture/status",
            timeout=2,
        ) as status:
            self.assertEqual(
                json.loads(status.read()),
                {
                    "schema": "worldstream/counter-deterministic-provider-status/v1",
                    "received_requests": 1,
                    "accepted_requests": 1,
                },
            )

    def test_http_provider_rejects_bad_credentials_without_echoing_them(self) -> None:
        body = {
            "content": {
                "instruction": "Select one offered action.",
                "observation": {},
                "offers": [{"offer_id": "increment-offer", "action_type": "increment"}],
            }
        }
        with self.assertRaises(urllib.error.HTTPError) as raised:
            self.request(body, b"different-model-token")

        payload = raised.exception.read().decode("utf-8")
        self.assertEqual(raised.exception.code, 401)
        self.assertNotIn("different-model-token", payload)
        self.assertNotIn(self.credential.decode("ascii"), payload)
        raised.exception.close()

    def test_rejects_an_unbounded_acceptance_recovery_delay(self) -> None:
        with self.assertRaisesRegex(ValueError, "response delay"):
            create_provider_server(
                "127.0.0.1", 0, self.credential_path, response_delay_ms=10_001
            )


if __name__ == "__main__":
    unittest.main()
