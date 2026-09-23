"""Provider clients for the Minesweeper comparison.

The LLM room calls OpenRouter; the JEV room calls TypeSafe System One. Both
receive the same ``agent_state`` and the same closed candidate set, so neither
engine has an information advantage. Keys stay in this server process.
"""

from __future__ import annotations

import json
import os
import threading
import time
import urllib.error
import urllib.request

OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"
OPENROUTER_MODELS_URL = "https://openrouter.ai/api/v1/models"
JEV_URL = "https://api.typesafe.ai/v1/systemone"
OPENROUTER_TIMEOUT_SECONDS = 30.0
JEV_TIMEOUT_SECONDS = 20.0
MAX_PROVIDER_RESPONSE_BYTES = 1_000_000
OPENROUTER_PROVIDER_SORT = "latency"
TITLE = "WorldStream Community Minesweeper"


class ProviderError(Exception):
    """A bounded, retryable provider failure with a machine-readable code."""


def openrouter_key() -> str | None:
    for name in ("OPENROUTER_API_KEY", "OPENROUTER_KEY", "openouterkey"):
        value = os.environ.get(name)
        if value:
            return value
    return None


def jev_key() -> str | None:
    value = os.environ.get("JEV_API_KEY")
    return value or None


CHEAP_PROMPT_CEILING_USD_PER_MILLION = 1.0
CHEAP_COMPLETION_CEILING_USD_PER_MILLION = 3.0
PREFERRED_MODELS = (
    "openai/gpt-oss-20b",
    "mistralai/mistral-small-24b-instruct-2501",
    "mistralai/mistral-nemo",
    "inclusionai/ling-3.0-flash",
    "deepseek/deepseek-v4-flash-0731",
    "amazon/nova-micro-v1",
    "meta-llama/llama-3.1-8b-instruct",
    "qwen/qwen3-30b-a3b-instruct-2507",
    "z-ai/glm-4.7-flash",
    "microsoft/phi-4",
)
CATALOG_CACHE_SECONDS = 300.0
CHEAP_MODELS_LIMIT = 30
_CATALOG_LOCK = threading.Lock()
_CATALOG_CACHE: dict[str, object] = {"at": 0.0, "models": []}


def _price_per_million(pricing: object, field: str) -> float | None:
    if not isinstance(pricing, dict):
        return None
    raw = pricing.get(field)
    if isinstance(raw, bool):
        return None
    try:
        value = float(raw)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return value * 1_000_000 if value >= 0 else None


def live_cheap_models() -> list[dict[str, str]]:
    """Cheap, fast, text-only OpenRouter models for the dropdown.

    Known fast instruction-followers are surfaced first with a star; the rest
    follow by price. The result is cached and falls back to an empty list so the
    caller can keep a static catalog.
    """
    now = time.monotonic()
    with _CATALOG_LOCK:
        cached = _CATALOG_CACHE.get("models")
        if isinstance(cached, list) and cached and now - float(_CATALOG_CACHE["at"]) < CATALOG_CACHE_SECONDS:
            return list(cached)  # type: ignore[arg-type]
    key = openrouter_key()
    request = urllib.request.Request(
        OPENROUTER_MODELS_URL,
        headers={"Authorization": f"Bearer {key}" if key else "", "X-Title": TITLE},
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            document = json.loads(response.read())
    except (OSError, TimeoutError, ValueError):
        return []
    entries = document.get("data") if isinstance(document, dict) else None
    if not isinstance(entries, list):
        return []
    scored: list[tuple[int, float, str, str]] = []
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        model_id = entry.get("id")
        if not isinstance(model_id, str) or not model_id or model_id.endswith(":free"):
            continue
        modality = entry.get("architecture", {}).get("modality") if isinstance(entry.get("architecture"), dict) else None
        if isinstance(modality, str) and "text" not in modality:
            continue
        prompt = _price_per_million(entry.get("pricing"), "prompt")
        completion = _price_per_million(entry.get("pricing"), "completion")
        if prompt is None or completion is None:
            continue
        if prompt > CHEAP_PROMPT_CEILING_USD_PER_MILLION:
            continue
        if completion > CHEAP_COMPLETION_CEILING_USD_PER_MILLION:
            continue
        label = entry.get("name") if isinstance(entry.get("name"), str) else model_id
        starred = 0 if model_id in PREFERRED_MODELS else 1
        scored.append((starred, prompt, model_id, f"{'★ ' if starred == 0 else ''}{label}"))
    scored.sort()
    models = [
        {"id": model_id, "label": label}
        for _, _, model_id, label in scored[:CHEAP_MODELS_LIMIT]
    ]
    with _CATALOG_LOCK:
        _CATALOG_CACHE["at"] = now
        _CATALOG_CACHE["models"] = models
    return models



def _http_json(
    url: str, body: dict[str, object], headers: dict[str, str], timeout: float
) -> dict[str, object]:
    encoded = json.dumps(body, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=encoded,
        headers={"Accept": "application/json", "Content-Type": "application/json", **headers},
        method="POST",
    )
    deadline = max(1.0, timeout)
    outcome: dict[str, object] = {}

    def perform() -> None:
        try:
            with urllib.request.urlopen(request, timeout=deadline) as response:
                outcome["raw"] = response.read(MAX_PROVIDER_RESPONSE_BYTES + 1)
        except urllib.error.HTTPError as error:
            outcome["http"] = error.code
        except (OSError, TimeoutError):
            outcome["failed"] = True

    worker = threading.Thread(target=perform, daemon=True, name="minesweeper-provider-call")
    worker.start()
    worker.join(deadline)
    if worker.is_alive():
        raise ProviderError("provider_request_timeout")
    if "http" in outcome:
        raise ProviderError(f"provider_request_failed_{outcome['http']}")
    if "failed" in outcome:
        raise ProviderError("provider_request_failed")
    raw = outcome.get("raw")
    if not isinstance(raw, bytes):
        raise ProviderError("provider_request_failed")
    if len(raw) > MAX_PROVIDER_RESPONSE_BYTES:
        raise ProviderError("provider_response_oversize")
    try:
        value = json.loads(raw)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ProviderError("provider_response_invalid") from error
    if not isinstance(value, dict):
        raise ProviderError("provider_response_invalid")
    return value


def llm_choose(
    state: dict[str, object],
    model: str,
    key: str,
    timeout: float = OPENROUTER_TIMEOUT_SECONDS,
) -> tuple[str | None, dict[str, object]]:
    """Ask the LLM for one cell among the offered candidates.

    Returns the chosen cell label (or None when the reply is unusable) plus
    telemetry. The caller validates membership in the candidate set.
    """
    instructions = (
        "You are an autonomous Minesweeper player. Read the board grid: '?' is a "
        "hidden cell, digits count adjacent mines, and the first reveal is always "
        "safe. The candidate list already excludes every cell proven to be a mine. "
        "Each candidate has provably_safe (a revealed number proves it cannot be a "
        "mine) and mine_risk (a computed 0..1 estimate, lower is safer). Prefer a "
        "provably_safe cell; when none is safe, reveal the candidate with the "
        "lowest mine_risk. "
        'Return exactly one JSON object: {"cell": "<label from candidates>", '
        '"confidence": <0..1>, "reason": "<safe|guess>", '
        '"rationale": "<one short sentence>"}.'
    )
    prompt = json.dumps(state, separators=(",", ":"))
    response = _http_json(
        OPENROUTER_URL,
        {
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "Return only one JSON decision object matching the offered candidates.",
                },
                {"role": "user", "content": f"{instructions}\n\n{prompt}"},
            ],
            "temperature": 0,
            "response_format": {"type": "json_object"},
            "provider": {"sort": OPENROUTER_PROVIDER_SORT},
        },
        {"Authorization": f"Bearer {key}", "X-Title": TITLE},
        min(max(1.0, timeout), OPENROUTER_TIMEOUT_SECONDS),
    )
    choices = response.get("choices")
    message = (
        choices[0].get("message")
        if isinstance(choices, list) and choices and isinstance(choices[0], dict)
        else None
    )
    content = message.get("content") if isinstance(message, dict) else None
    if isinstance(content, list):
        content = "".join(
            item.get("text", "")
            for item in content
            if isinstance(item, dict) and isinstance(item.get("text"), str)
        )
    if not isinstance(content, str):
        raise ProviderError("openrouter_choice_missing")
    try:
        decision = json.loads(content)
    except json.JSONDecodeError as error:
        raise ProviderError("openrouter_decision_invalid") from error
    if not isinstance(decision, dict):
        raise ProviderError("openrouter_decision_invalid")
    cell = decision.get("cell")
    metadata: dict[str, object] = {
        "provider_model": response.get("model") if isinstance(response.get("model"), str) else model,
        "confidence": _bounded_confidence(decision.get("confidence")),
        "reason": decision.get("reason") if isinstance(decision.get("reason"), str) else None,
        "rationale": decision.get("rationale") if isinstance(decision.get("rationale"), str) else None,
        "usage": response.get("usage") if isinstance(response.get("usage"), dict) else None,
    }
    return (cell if isinstance(cell, str) else None), metadata


def jev_choose(
    state: dict[str, object],
    criteria: dict[str, str],
    model: str,
    key: str,
    timeout: float = JEV_TIMEOUT_SECONDS,
) -> tuple[str | None, dict[str, object]]:
    """Ask JEV to choose one cell from the closed candidate set.

    JEV receives the same board state and a closed Choice whose criteria are the
    offered cells. It never answers a separate safety question.
    """
    response = _http_json(
        JEV_URL,
        {
            "state": state,
            "model": model,
            "questions": {
                "cell": {
                    "type": "choice",
                    "instructions": (
                        "Choose the single hidden cell to reveal next. The offered "
                        "cells already exclude every cell proven to be a mine. Each "
                        "criterion states whether a revealed number proves the cell "
                        "safe, or gives a computed mine-risk estimate where no proof "
                        "exists. Prefer a provably safe cell; when no cell is provably "
                        "safe, choose the cell with the lowest mine risk."
                    ),
                    "criteria": criteria,
                }
            },
        },
        {"Authorization": f"Bearer {key}"},
        min(max(1.0, timeout), JEV_TIMEOUT_SECONDS),
    )
    answers = response.get("answers")
    answer = answers.get("cell") if isinstance(answers, dict) else None
    choice = answer.get("choice") if isinstance(answer, dict) else None
    if not isinstance(choice, str):
        raise ProviderError("jev_choice_missing")
    metadata: dict[str, object] = {
        "provider_model": response.get("model") if isinstance(response.get("model"), str) else model,
        "confidence": _bounded_confidence(answer.get("confidence")) if isinstance(answer, dict) else None,
        "probabilities": answer.get("probabilities") if isinstance(answer, dict) else None,
        "usage": response.get("usage") if isinstance(response.get("usage"), dict) else None,
    }
    return choice, metadata


def _bounded_confidence(value: object) -> float | None:
    if isinstance(value, bool):
        return None
    try:
        number = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return max(0.0, min(1.0, number))
