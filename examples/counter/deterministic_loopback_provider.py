#!/usr/bin/env python3
"""Serve the deterministic local Counter development provider.

The fixture is intentionally loopback-only and accepts no ambient credentials,
external network configuration, or provider account settings. Its owner-only
credential file is read only to verify requests from the managed reference
host; it is never included in a response, diagnostic, or log message.
"""

from __future__ import annotations

import argparse
import hmac
import ipaddress
import json
import os
import stat
import sys
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

FIXTURE_LABEL = "worldstream-counter-deterministic-loopback-v1"
MAX_REQUEST_BYTES = 256 * 1024
MAX_OFFERS = 32
MAX_RESPONSE_DELAY_MS = 10_000


class ProviderConfigurationError(ValueError):
    """A bounded local fixture configuration failure."""


def _loopback_address(address: str) -> str:
    try:
        parsed = ipaddress.ip_address(address)
    except ValueError as error:
        raise ProviderConfigurationError(
            "provider bind address must be a loopback IP"
        ) from error
    if not parsed.is_loopback:
        raise ProviderConfigurationError("provider bind address must be a loopback IP")
    return str(parsed)


def load_owner_only_credential(path: Path) -> bytes:
    """Read one bounded fixture credential from an owner-only regular file."""

    try:
        metadata = path.lstat()
    except OSError as error:
        raise ProviderConfigurationError(
            "provider credential file is unavailable"
        ) from error
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise ProviderConfigurationError(
            "provider credential file must be a regular file"
        )
    if metadata.st_mode & 0o077:
        raise ProviderConfigurationError("provider credential file must be owner-only")
    if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
        raise ProviderConfigurationError("provider credential file has the wrong owner")
    try:
        credential = path.read_bytes()
    except OSError as error:
        raise ProviderConfigurationError(
            "provider credential file is unavailable"
        ) from error
    if (
        not credential
        or len(credential) > 16 * 1024
        or any(byte in b"\r\n" for byte in credential)
    ):
        raise ProviderConfigurationError("provider credential material is invalid")
    return credential


def _completion(content: object) -> dict[str, object]:
    if not isinstance(content, dict):
        raise TypeError("content must be an object")
    if not isinstance(content.get("instruction"), str):
        raise TypeError("content instruction is invalid")
    if not isinstance(content.get("observation"), dict):
        raise TypeError("content observation is invalid")
    offered = content.get("offers")
    # The managed host forwards the complete assignment-MCP offer DTO, rather
    # than flattening its content before the model boundary.  Keep the older
    # direct-list shape for focused provider tests, but select only from the
    # exact bounded offers list in either accepted production shape.
    if isinstance(offered, dict):
        offers = offered.get("offers")
        if not isinstance(offered.get("schema"), str):
            raise TypeError("content offer envelope is invalid")
    else:
        offers = offered
    if not isinstance(offers, list) or len(offers) > MAX_OFFERS:
        raise ValueError("content offers are invalid")
    increments = [
        offer
        for offer in offers
        if isinstance(offer, dict)
        and offer.get("action_type") == "increment"
        and isinstance(offer.get("offer_id"), str)
        and offer["offer_id"]
    ]
    if len(increments) != 1:
        raise ValueError("content must contain one exact increment offer")
    selected = {"offer_id": increments[0]["offer_id"], "payload": {}}
    return {
        "id": "deterministic-counter-completion",
        "object": "chat.completion",
        "created": 0,
        "model": FIXTURE_LABEL,
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": json.dumps(
                        selected, sort_keys=True, separators=(",", ":")
                    ),
                },
                "finish_reason": "stop",
            }
        ],
    }


def _request_content(request: dict[str, object]) -> object:
    if "content" in request:
        return request["content"]
    messages = request.get("messages")
    if (
        not isinstance(messages, list)
        or len(messages) != 1
        or not isinstance(messages[0], dict)
        or messages[0].get("role") != "user"
        or not isinstance(messages[0].get("content"), str)
    ):
        raise ValueError("request content is invalid")
    return json.loads(messages[0]["content"])


def create_provider_server(
    bind_address: str, port: int, credential_file: Path, *, response_delay_ms: int = 0
) -> ThreadingHTTPServer:
    """Create a loopback-only OpenAI-compatible deterministic provider."""

    address = _loopback_address(bind_address)
    if not 0 <= port <= 65535:
        raise ProviderConfigurationError("provider port is outside the accepted range")
    if not 0 <= response_delay_ms <= MAX_RESPONSE_DELAY_MS:
        raise ProviderConfigurationError(
            "provider response delay is outside the accepted range"
        )
    credential = load_owner_only_credential(credential_file)

    class DeterministicProviderHandler(BaseHTTPRequestHandler):
        server_version = "WorldStreamDeterministicProvider/1"
        sys_version = ""

        def log_message(self, _format: str, *_arguments: object) -> None:
            return

        def _send_json(self, status: HTTPStatus, payload: dict[str, object]) -> None:
            encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode(
                "utf-8"
            )
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def _error(self, status: HTTPStatus, code: str) -> None:
            self._send_json(status, {"error": {"code": code}})

        def do_GET(self) -> None:
            # This development-only diagnostic deliberately carries aggregate
            # counts, never a model request, response, authorization value, or
            # provider configuration.  It lets the acceptance harness prove a
            # bounded model boundary without retaining private content.
            if self.path != "/fixture/status":
                self._error(HTTPStatus.NOT_FOUND, "not_found")
                return
            self._send_json(
                HTTPStatus.OK,
                {
                    "schema": "worldstream/counter-deterministic-provider-status/v1",
                    "received_requests": self.server.received_requests,
                    "accepted_requests": self.server.accepted_requests,
                },
            )

        def do_POST(self) -> None:
            if self.path != "/v1/chat/completions":
                self._error(HTTPStatus.NOT_FOUND, "not_found")
                return
            supplied = self.headers.get("Authorization")
            expected = b"Bearer " + credential
            if supplied is None or not hmac.compare_digest(
                supplied.encode("utf-8", "surrogateescape"), expected
            ):
                self._error(HTTPStatus.UNAUTHORIZED, "unauthorized")
                return
            try:
                content_length = int(self.headers.get("Content-Length", ""))
            except ValueError:
                self._error(HTTPStatus.BAD_REQUEST, "invalid_content_length")
                return
            if not 0 <= content_length <= MAX_REQUEST_BYTES:
                self._error(HTTPStatus.REQUEST_ENTITY_TOO_LARGE, "request_too_large")
                return
            try:
                request = json.loads(self.rfile.read(content_length))
                if not isinstance(request, dict):
                    raise TypeError("request must be an object")
                response = _completion(_request_content(request))
            except (TypeError, UnicodeDecodeError, ValueError, json.JSONDecodeError):
                self._error(
                    HTTPStatus.UNPROCESSABLE_ENTITY, "invalid_completion_request"
                )
                return
            self.server.received_requests += 1
            if response_delay_ms:
                # The acceptance lane uses this bounded development-only pause
                # to stop a host while its Activation is leased.  It neither
                # changes the selected offer nor accepts remote configuration.
                time.sleep(response_delay_ms / 1000)
            self.server.accepted_requests += 1
            self._send_json(HTTPStatus.OK, response)

    try:
        server = ThreadingHTTPServer((address, port), DeterministicProviderHandler)
        server.received_requests = 0
        server.accepted_requests = 0
        return server
    except OSError as error:
        raise ProviderConfigurationError(
            "deterministic provider port is occupied or unavailable; choose an unused local port"
        ) from error


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--credential-file", required=True, type=Path)
    parser.add_argument("--response-delay-ms", type=int, default=0)
    args = parser.parse_args(arguments)
    try:
        server = create_provider_server(
            args.bind,
            args.port,
            args.credential_file,
            response_delay_ms=args.response_delay_ms,
        )
    except ProviderConfigurationError as error:
        print(str(error), file=sys.stderr)
        return 2
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
