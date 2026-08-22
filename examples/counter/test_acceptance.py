from __future__ import annotations

import sys
import tempfile
from pathlib import Path

from worldstream_sdk import ProtocolError

sys.path.insert(0, str(Path(__file__).parent))

from run_live_acceptance import (
    AcceptanceFailure,
    _owner_only_regular_file,
    canonical_hash,
    safe_error,
)


def test_canonical_hash_is_order_independent() -> None:
    assert canonical_hash({"b": 2, "a": 1}) == canonical_hash({"a": 1, "b": 2})


def test_safe_error_contains_no_protocol_message_or_secret_material() -> None:
    error = ProtocolError(
        "forbidden",
        "bearer wsb1:" + "a" * 64 + " is not authorized",
        False,
        {"authorization": "wsb1:" + "a" * 64},
    )
    safe = safe_error(error)
    assert safe == {"code": "forbidden", "retryable": False}
    assert "wsb1:" not in str(safe)


def test_postgresql_dsn_file_must_be_owner_only_and_not_a_symlink() -> None:
    with tempfile.TemporaryDirectory(prefix="worldstream-counter-dsn-") as name:
        root = Path(name)
        dsn = root / "runtime.dsn"
        dsn.write_text("credential material is intentionally opaque", encoding="utf-8")
        dsn.chmod(0o600)
        assert _owner_only_regular_file(dsn) == dsn.absolute()

        dsn.chmod(0o640)
        try:
            _owner_only_regular_file(dsn)
        except AcceptanceFailure as error:
            assert error.code == "postgresql_dsn_file_not_owner_only"
        else:
            raise AssertionError("group-readable DSN was accepted")

        dsn.chmod(0o600)
        link = root / "runtime-link.dsn"
        link.symlink_to(dsn)
        try:
            _owner_only_regular_file(link)
        except AcceptanceFailure as error:
            assert error.code == "postgresql_dsn_file_not_regular"
        else:
            raise AssertionError("symlink DSN was accepted")
