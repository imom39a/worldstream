"""Focused checks for the JEV-judge comparison adapter and its cheap catalog."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from examples.tower_of_hanoi import comparison_canvas_v2 as v2


def _document() -> dict[str, object]:
    return {
        "data": [
            {
                "id": "vendor/expensive",
                "name": "Expensive",
                "pricing": {"prompt": "0.00001", "completion": "0.00003"},
                "architecture": {"modality": "text->text"},
            },
            {
                "id": "vendor/cheap",
                "name": "Cheap",
                "pricing": {"prompt": "0.0000001", "completion": "0.0000002"},
                "architecture": {"modality": "text->text"},
            },
            {
                "id": "openai/gpt-oss-20b",
                "name": "gpt-oss-20b",
                "pricing": {"prompt": "0.0000005", "completion": "0.0000015"},
                "architecture": {"modality": "text->text"},
            },
            {
                "id": "vendor/free-cheap:free",
                "name": "Free Cheap",
                "pricing": {"prompt": "0", "completion": "0"},
                "architecture": {"modality": "text->text"},
            },
            {
                "id": "vendor/audio",
                "name": "Audio",
                "pricing": {"prompt": "0.0000001", "completion": "0.0000001"},
                "architecture": {"modality": "text->audio"},
            },
            {
                "id": "vendor/router",
                "name": "Router",
                "pricing": {"prompt": "-1", "completion": "-1"},
                "architecture": {"modality": "text->text"},
            },
        ]
    }


def _response(payload: dict[str, object]) -> MagicMock:
    responder = MagicMock()
    responder.__enter__.return_value.read.return_value = json.dumps(payload).encode()
    return responder


class CheapCatalogTests(unittest.TestCase):
    def test_live_catalog_keeps_only_cheap_text_models(self) -> None:
        with patch.object(
            v2.urllib.request, "urlopen", return_value=_response(_document())
        ):
            models = v2._live_cheap_models()
        # The preferred fast model is surfaced first even though it costs more.
        self.assertEqual(
            [item["id"] for item in models],
            ["openai/gpt-oss-20b", "vendor/cheap"],
        )
        self.assertTrue(models[0]["label"].startswith("★ "))

    def test_catalog_falls_back_when_the_provider_is_unreachable(self) -> None:
        with v2._CATALOG_LOCK:
            v2._CATALOG_CACHE["at"] = 0.0
            v2._CATALOG_CACHE["models"] = []
        with patch.object(v2.urllib.request, "urlopen", side_effect=OSError("offline")):
            models = v2.cheap_models()
        self.assertTrue(models)
        self.assertEqual(models[0]["id"], v2.FALLBACK_MODELS[0]["id"])


class RunValidationTests(unittest.TestCase):
    def _run(self) -> v2.ComparisonRunV2:
        return v2.ComparisonRunV2(
            Path("/tmp/worldstream-v2-test"), v2.base.ComparisonBroadcast()
        )

    def test_rejects_non_numeric_judge_threshold(self) -> None:
        with self.assertRaises(TypeError):
            self._run().start({"judge_threshold": "high"})

    def test_rejects_out_of_range_judge_threshold(self) -> None:
        with self.assertRaises(ValueError):
            self._run().start({"judge_threshold": 2.0})

    def test_start_builds_only_string_arguments(self) -> None:
        run = v2.ComparisonRunV2(
            Path(tempfile.mkdtemp()), v2.ComparisonBroadcastV2()
        )
        with patch.object(v2.subprocess, "Popen") as popen, patch.object(
            v2.threading, "Thread"
        ):
            run.start(
                {
                    "model": "openai/gpt-oss-20b",
                    "solver_count": 1,
                    "disks": 3,
                    "duration_seconds": 300,
                }
            )
        self.assertTrue(popen.call_args_list)
        for call in popen.call_args_list:
            command = call.args[0]
            self.assertTrue(
                all(isinstance(argument, str) for argument in command),
                command,
            )


if __name__ == "__main__":
    unittest.main()