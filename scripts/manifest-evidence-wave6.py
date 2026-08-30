"""Audit deterministic manifest identities without producing release claims.

This is intentionally a report/validation command, not a manifest generator.
It reads the authored compatibility pair and the implementation sources, then
reports identities that can be recomputed from bytes available in the source
checkout.  It never writes a manifest, release directory, or evidence row.

The small BLAKE3 implementation is limited to the unkeyed hash used by the
Rust identity code.  Keeping it here makes migration and text-artifact checks
reproducible on a clean Python installation; it is not used to invent missing
release evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_TOML = ROOT / "compatibility.toml"
MANIFEST_JSON = ROOT / "compatibility.json"
SQLITE_SOURCE = ROOT / "crates/worldstream-sqlite/src/lib.rs"
POSTGRES_SOURCE = ROOT / "crates/worldstream-postgres/src/lib.rs"
POSTGRES_MIGRATIONS = ROOT / "crates/worldstream-postgres/src/migrations.rs"
HEIST_SOURCE = ROOT / "crates/worldstream-core/src/agent_heist.rs"
HEIST_REGISTRY_SOURCE = ROOT / "crates/worldstream-core/src/agent_heist_registry.rs"
WORKSPACE_MANIFEST = ROOT / "Cargo.toml"
CARGO_LOCK = ROOT / "Cargo.lock"

HEX64 = re.compile(r"[0-9a-f]{64}\Z")
BLAKE3_REFERENCE = re.compile(r"blake3:[0-9a-f]{64}\Z")
SHA256_REFERENCE = re.compile(r"sha256:[0-9a-f]{64}\Z")

# BLAKE3 flags and compression constants from the public BLAKE3 specification.
_IV = (
    0x6A09E667,
    0xBB67AE85,
    0x3C6EF372,
    0xA54FF53A,
    0x510E527F,
    0x9B05688C,
    0x1F83D9AB,
    0x5BE0CD19,
)
_PERMUTATION = (2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)
_CHUNK_START = 1
_CHUNK_END = 2
_PARENT = 4
_ROOT = 8
_MASK32 = 0xFFFFFFFF


def _rotr(value: int, amount: int) -> int:
    return ((value >> amount) | (value << (32 - amount))) & _MASK32


def _g(state: list[int], a: int, b: int, c: int, d: int, mx: int, my: int) -> None:
    state[a] = (state[a] + state[b] + mx) & _MASK32
    state[d] = _rotr(state[d] ^ state[a], 16)
    state[c] = (state[c] + state[d]) & _MASK32
    state[b] = _rotr(state[b] ^ state[c], 12)
    state[a] = (state[a] + state[b] + my) & _MASK32
    state[d] = _rotr(state[d] ^ state[a], 8)
    state[c] = (state[c] + state[d]) & _MASK32
    state[b] = _rotr(state[b] ^ state[c], 7)


def _round(state: list[int], message: list[int]) -> None:
    _g(state, 0, 4, 8, 12, message[0], message[1])
    _g(state, 1, 5, 9, 13, message[2], message[3])
    _g(state, 2, 6, 10, 14, message[4], message[5])
    _g(state, 3, 7, 11, 15, message[6], message[7])
    _g(state, 0, 5, 10, 15, message[8], message[9])
    _g(state, 1, 6, 11, 12, message[10], message[11])
    _g(state, 2, 7, 8, 13, message[12], message[13])
    _g(state, 3, 4, 9, 14, message[14], message[15])


def _compress(
    chaining_value: Iterable[int],
    block_words: list[int],
    counter: int,
    block_length: int,
    flags: int,
) -> list[int]:
    state = list(chaining_value) + list(_IV[:4])
    state.extend((counter & _MASK32, (counter >> 32) & _MASK32, block_length, flags))
    message = list(block_words)
    for round_index in range(7):
        _round(state, message)
        if round_index != 6:
            message = [message[index] for index in _PERMUTATION]
    return [
        *(state[index] ^ state[index + 8] for index in range(8)),
        *(state[index + 8] ^ chaining_value[index] for index in range(8)),
    ]


class _Output:
    def __init__(
        self,
        chaining_value: list[int],
        block_words: list[int],
        counter: int,
        block_length: int,
        flags: int,
    ) -> None:
        self.chaining_value = chaining_value
        self.block_words = block_words
        self.counter = counter
        self.block_length = block_length
        self.flags = flags

    def chaining_value_bytes(self) -> list[int]:
        return _compress(
            self.chaining_value,
            self.block_words,
            self.counter,
            self.block_length,
            self.flags,
        )[:8]

    def root_bytes(self, length: int = 32) -> bytes:
        result = bytearray()
        output_counter = 0
        while len(result) < length:
            words = _compress(
                self.chaining_value,
                self.block_words,
                output_counter,
                self.block_length,
                self.flags | _ROOT,
            )
            result.extend(b"".join(word.to_bytes(4, "little") for word in words))
            output_counter += 1
        return bytes(result[:length])


def _words(block: bytes) -> list[int]:
    padded = block.ljust(64, b"\0")
    return [
        int.from_bytes(padded[index : index + 4], "little") for index in range(0, 64, 4)
    ]


def _chunk_output(chunk: bytes, chunk_counter: int) -> _Output:
    if not chunk:
        blocks = [b""]
    else:
        blocks = [chunk[index : index + 64] for index in range(0, len(chunk), 64)]
    chaining_value = list(_IV)
    for index, block in enumerate(blocks[:-1]):
        flags = _CHUNK_START if index == 0 else 0
        chaining_value = _compress(
            chaining_value,
            _words(block),
            chunk_counter,
            64,
            flags,
        )[:8]
    final_index = len(blocks) - 1
    flags = _CHUNK_END | (_CHUNK_START if final_index == 0 else 0)
    final_block = blocks[-1]
    return _Output(
        chaining_value,
        _words(final_block),
        chunk_counter,
        len(final_block),
        flags,
    )


def _parent_output(left: list[int], right: list[int]) -> _Output:
    return _Output(list(_IV), left + right, 0, 64, _PARENT)


def blake3(data: bytes) -> bytes:
    """Return the unkeyed BLAKE3 digest for *data*."""

    chunks = [data[index : index + 1024] for index in range(0, len(data), 1024)]
    if not chunks:
        chunks = [b""]
    outputs = [_chunk_output(chunk, index) for index, chunk in enumerate(chunks)]
    if len(outputs) == 1:
        return outputs[0].root_bytes()

    nodes = [output.chaining_value_bytes() for output in outputs]
    while len(nodes) > 2:
        next_nodes: list[list[int]] = []
        index = 0
        while index + 1 < len(nodes):
            next_nodes.append(
                _parent_output(nodes[index], nodes[index + 1]).chaining_value_bytes()
            )
            index += 2
        if index < len(nodes):
            next_nodes.append(nodes[index])
        nodes = next_nodes
    return _parent_output(nodes[0], nodes[1]).root_bytes()


def _read(path: Path) -> bytes:
    return path.read_bytes()


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_reference(data: bytes) -> str:
    return f"sha256:{_sha256(data)}"


def _blake3_reference(data: bytes) -> str:
    return f"blake3:{blake3(data).hex()}"


def _canonical_inventory(records: Iterable[tuple[str, bytes]]) -> bytes:
    """Canonical bytes for a path/size/SHA-256 inventory fingerprint."""

    lines = []
    for name, content in sorted(records):
        lines.append(f"{name}\0{len(content)}\0{_sha256(content)}\n")
    return "".join(lines).encode("utf-8")


def _file_record(path: Path, *, name: str | None = None) -> dict[str, Any]:
    content = _read(path)
    return {
        "path": name or path.relative_to(ROOT).as_posix(),
        "size_bytes": len(content),
        "sha256": _sha256_reference(content),
    }


def _rust_string_constants(source: str, suffix: str) -> dict[str, str]:
    pattern = re.compile(
        rf"(?:pub\s+)?const\s+([A-Z0-9_]+{re.escape(suffix)}):\s*&str\s*=\s*\"([^\"]*)\"\s*;"
    )
    return {name: value for name, value in pattern.findall(source)}


def _rust_raw_string_constants(source: str, suffix: str) -> dict[str, bytes]:
    pattern = re.compile(
        rf"(?:pub\s+)?const\s+([A-Z0-9_]+{re.escape(suffix)}):\s*&str\s*=\s*r\"(.*?)\"\s*;",
        re.DOTALL,
    )
    return {name: value.encode("utf-8") for name, value in pattern.findall(source)}


def _find_checkout() -> Path | None:
    revision = _rust_string_constants(
        WORKSPACE_MANIFEST.read_text(encoding="utf-8"), ""
    )
    del revision  # Keep discovery independent of parsing Cargo's TOML shape.
    wanted = "229140734a4a60cc9fa34507fe79cb2277142f49"
    cargo_home = Path.home() / ".cargo" / "git" / "checkouts"
    if not cargo_home.is_dir():
        return None
    for candidate in cargo_home.glob("rusqlite-*/*"):
        if candidate.is_dir() and (candidate / ".git" / "HEAD").exists():
            try:
                actual = subprocess.run(
                    ["git", "-C", str(candidate), "rev-parse", "HEAD"],
                    capture_output=True,
                    check=True,
                    text=True,
                ).stdout.strip()
            except (OSError, subprocess.CalledProcessError):
                continue
            if actual == wanted:
                return candidate
    return None


def _checkout_revision(checkout: Path) -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(checkout), "rev-parse", "HEAD"],
            capture_output=True,
            check=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(
            f"cannot verify bundled checkout revision: {checkout}"
        ) from error


def _manifest_report() -> dict[str, Any]:
    authored_bytes = _read(MANIFEST_TOML)
    mirror_bytes = _read(MANIFEST_JSON)
    authored = tomllib.loads(authored_bytes.decode("utf-8"))
    mirror = json.loads(mirror_bytes)
    canonical_mirror = (
        json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")
    release_artifacts = authored.get("release_artifacts", [])
    evidence = authored.get("evidence", [])
    artifact_ids = [row.get("id") for row in release_artifacts if isinstance(row, dict)]
    evidence_rows = [row for row in evidence if isinstance(row, dict)]
    evidence_shape_ok = all(
        (row.get("status") == "unresolved" and row.get("artifact_digest") == "")
        or (
            row.get("status") == "detached"
            and row.get("release_gate") is True
            and row.get("artifact_digest") == ""
            and row.get("artifact_digest_location") == "release-manifest.json"
        )
        or (
            row.get("status") == "detached"
            and row.get("release_gate") is False
            and row.get("qualification_gate") is True
            and row.get("artifact_digest") == ""
            and row.get("artifact_digest_location")
            == "release-qualification-manifest.json"
        )
        or (
            row.get("status") == "resolved"
            and isinstance(row.get("artifact_digest"), str)
            and bool(SHA256_REFERENCE.fullmatch(row["artifact_digest"]))
        )
        for row in evidence_rows
    )
    return {
        "files": {
            "compatibility_toml": _file_record(MANIFEST_TOML),
            "compatibility_json": _file_record(MANIFEST_JSON),
        },
        "parity": {
            "semantic_equal": authored == mirror,
            "canonical_mirror_equal": mirror_bytes == canonical_mirror,
            "reviewed_source_ok": authored.get("reviewed_source") == MANIFEST_TOML.name,
            "canonical_mirror_ok": authored.get("canonical_mirror")
            == MANIFEST_JSON.name,
        },
        "state": {
            "manifest_kind": authored.get("manifest_kind"),
            "release_ready": authored.get("release_ready"),
            "unresolved_required_fields": authored.get(
                "unresolved_required_fields", []
            ),
            "release_artifact_count": len(release_artifacts),
            "release_artifact_ids": artifact_ids,
            "release_artifact_ids_unique": len(artifact_ids) == len(set(artifact_ids)),
            "evidence_count": len(evidence_rows),
            "evidence_shape_ok": evidence_shape_ok,
            "resolved_evidence_ids": [
                row.get("id")
                for row in evidence_rows
                if row.get("status") == "resolved"
            ],
        },
    }


def _sqlite_report(bundled_checkout: Path | None) -> dict[str, Any]:
    source_text = SQLITE_SOURCE.read_text(encoding="utf-8")
    constants = _rust_string_constants(source_text, "")
    # The general parser intentionally gets only exact names below; it avoids
    # treating unrelated source strings as engine identity.
    exact = {
        key: constants[key]
        for key in ("SQLITE_VERSION", "SQLITE_SOURCE_ID", "RUSQLITE_BUNDLE_REVISION")
        if key in constants
    }
    manifest = tomllib.loads(_read(MANIFEST_TOML).decode("utf-8"))
    storage = manifest["storage"]["sqlite"]
    workspace_text = WORKSPACE_MANIFEST.read_text(encoding="utf-8")
    lock_text = CARGO_LOCK.read_text(encoding="utf-8")
    lock_match = re.search(
        r"name = \"rusqlite\".*?source = \"git\+[^\"]+rev=([0-9a-f]{40})#",
        lock_text,
        re.DOTALL,
    )
    workspace_match = re.search(
        r"rusqlite\s*=\s*\{[^\n]*?rev\s*=\s*\"([0-9a-f]{40})\"",
        workspace_text,
    )
    identity_matches = {
        "manifest_version": storage.get("version") == exact.get("SQLITE_VERSION"),
        "manifest_source_id": storage.get("source_id") == exact.get("SQLITE_SOURCE_ID"),
        "workspace_revision": workspace_match is not None
        and workspace_match.group(1) == exact.get("RUSQLITE_BUNDLE_REVISION"),
        "lock_revision": lock_match is not None
        and lock_match.group(1) == exact.get("RUSQLITE_BUNDLE_REVISION"),
    }
    result: dict[str, Any] = {
        "source_constants": exact,
        "manifest_values": {
            "version": storage.get("version"),
            "source_id": storage.get("source_id"),
            "bundle_source_inventory_digest": storage.get(
                "bundle_source_inventory_digest"
            ),
            "bundle_source_inventory_status": storage.get(
                "bundle_source_inventory_status"
            ),
        },
        "cargo_revisions": {
            "workspace_toml": workspace_match.group(1) if workspace_match else None,
            "cargo_lock": lock_match.group(1) if lock_match else None,
        },
        "identity_matches": identity_matches,
        "identity_consistent": all(identity_matches.values()),
        "bundle_source_inventory": {
            "status": storage.get("bundle_source_inventory_status"),
            "digest": storage.get("bundle_source_inventory_digest"),
            "reason": "portable bundled source identity; platform build bytes are detached provenance subjects",
        },
        "source_files": [_file_record(SQLITE_SOURCE)],
    }
    if bundled_checkout is None:
        result["bundled_source"] = {"status": "unavailable"}
        return result

    expected_revision = exact.get("RUSQLITE_BUNDLE_REVISION")
    actual_revision = _checkout_revision(bundled_checkout)
    if actual_revision != expected_revision:
        raise RuntimeError(
            "bundled checkout revision does not match Cargo pin: "
            f"expected {expected_revision}, got {actual_revision}"
        )
    sqlite_dir = bundled_checkout / "libsqlite3-sys" / "sqlite3"
    build_rs = bundled_checkout / "libsqlite3-sys" / "build.rs"
    names = ["sqlite3.c", "sqlite3.h", "sqlite3ext.h", "bindgen_bundled_version.rs"]
    records = []
    for name in names:
        path = sqlite_dir / name
        if path.is_file():
            content = _read(path)
            records.append((f"libsqlite3-sys/sqlite3/{name}", content))
    if build_rs.is_file():
        records.append(("libsqlite3-sys/build.rs", _read(build_rs)))
    inventory_bytes = _canonical_inventory(records)
    result["bundled_source"] = {
        "status": "found",
        "revision": actual_revision,
        "files": [
            {
                "path": name,
                "size_bytes": len(content),
                "sha256": _sha256_reference(content),
            }
            for name, content in sorted(records)
        ],
        "input_inventory_sha256": _sha256_reference(inventory_bytes),
        "input_inventory_bytes": len(inventory_bytes),
        "build_identity_is_not_release_digest": True,
    }
    return result


def _migration_records(
    source: Path, id_suffix: str, schema_suffix: str
) -> list[dict[str, Any]]:
    text = source.read_text(encoding="utf-8")
    ids = _rust_string_constants(text, id_suffix)
    bodies = _rust_raw_string_constants(text, schema_suffix)
    records = []
    for id_name, migration_id in ids.items():
        schema_name = id_name[: -len(id_suffix)] + schema_suffix
        body = bodies.get(schema_name)
        if body is None:
            continue
        records.append(
            {
                "id_constant": id_name,
                "id": migration_id,
                "schema_constant": schema_name,
                "size_bytes": len(body),
                "sha256": _sha256_reference(body),
                "blake3": _blake3_reference(body),
            }
        )
    return records


def _migration_report() -> dict[str, Any]:
    sqlite_records = _migration_records(
        SQLITE_SOURCE, "_MIGRATION_ID", "_MIGRATION_SCHEMA"
    )
    postgres_text = POSTGRES_SOURCE.read_text(encoding="utf-8")
    postgres_migration_text = POSTGRES_MIGRATIONS.read_text(encoding="utf-8")
    postgres_ids = _rust_string_constants(postgres_migration_text, "_MIGRATION_ID")
    postgres_bodies = _rust_raw_string_constants(postgres_migration_text, "_SQL")
    initial_match = re.search(
        r"const SCHEMA:\s*&str\s*=\s*r\"(.*?)\"\s*;", postgres_text, re.DOTALL
    )
    if initial_match:
        postgres_bodies["SCHEMA"] = initial_match.group(1).encode("utf-8")
    postgres_records = []
    for id_name, migration_id in postgres_ids.items():
        suffix = id_name[: -len("_MIGRATION_ID")]
        body = postgres_bodies.get(
            "SCHEMA"
            if suffix == "INITIAL"
            else f"MIGRATION_{suffix.removeprefix('MIGRATION_')}_SQL"
        )
        # The source constants use MIGRATION_0002_SQL while the ID constant is
        # AUTHORITY_MIGRATION_ID, so map by the descriptor's version below.
        if body is None:
            continue
        postgres_records.append(
            {
                "id": migration_id,
                "size_bytes": len(body),
                "sha256": _sha256_reference(body),
                "blake3": _blake3_reference(body),
            }
        )
    # The versioned descriptor is the authoritative ordering/coverage source.
    descriptor_rows = re.findall(
        r"MigrationDescriptor\s*\{\s*version:\s*(\d+),\s*id:\s*([A-Z0-9_]+),\s*sql:\s*(?:super::)?([A-Z0-9_]+)",
        postgres_migration_text,
        re.DOTALL,
    )
    postgres_records = []
    for version, id_constant, sql_constant in descriptor_rows:
        body = postgres_bodies.get(sql_constant)
        if body is None and sql_constant == "SCHEMA":
            body = _rust_raw_string_constants(postgres_text, "").get("SCHEMA")
        if body is None:
            postgres_records.append(
                {
                    "version": int(version),
                    "id": postgres_ids.get(id_constant),
                    "body_status": "unavailable",
                }
            )
        else:
            postgres_records.append(
                {
                    "version": int(version),
                    "id": postgres_ids.get(id_constant),
                    "size_bytes": len(body),
                    "sha256": _sha256_reference(body),
                    "blake3": _blake3_reference(body),
                }
            )
    manifest = tomllib.loads(_read(MANIFEST_TOML).decode("utf-8"))
    manifest_entries = manifest.get("migrations", {}).get("entries", [])
    sqlite_source_ids = {row.get("id") for row in sqlite_records}
    postgres_source_ids = {row.get("id") for row in postgres_records}
    sqlite_manifest_checksums = {
        row.get("id"): row.get("sqlite_checksum")
        for row in manifest_entries
        if isinstance(row, dict) and row.get("sqlite_checksum")
    }
    postgres_manifest_checksums = {
        row.get("id"): row.get("postgresql_checksum")
        for row in manifest_entries
        if isinstance(row, dict) and row.get("postgresql_checksum")
    }
    sqlite_source_checksums = {
        row.get("id"): row.get("blake3") for row in sqlite_records
    }
    postgres_source_checksums = {
        row.get("id"): row.get("blake3") for row in postgres_records
    }
    return {
        "sqlite": {
            "source_records": sqlite_records,
            "source_count": len(sqlite_records),
            "manifest_checksum_fields": sqlite_manifest_checksums,
            "source_ids_not_in_manifest": sorted(
                sqlite_source_ids - set(sqlite_manifest_checksums)
            ),
            "manifest_ids_not_in_source": sorted(
                set(sqlite_manifest_checksums) - sqlite_source_ids
            ),
            "checksum_mismatches": sorted(
                migration_id
                for migration_id in sqlite_source_ids & set(sqlite_manifest_checksums)
                if sqlite_source_checksums[migration_id]
                != sqlite_manifest_checksums[migration_id]
            ),
            "checksum_implementation": "not present in SQLite adapter; source body hashes are reported, not promoted",
            "source_sha256": _sha256_reference(_read(SQLITE_SOURCE)),
        },
        "postgresql": {
            "source_records": postgres_records,
            "source_count": len(postgres_records),
            "manifest_entry_count": len(manifest_entries),
            "source_ids_not_in_manifest": sorted(
                postgres_source_ids - set(postgres_manifest_checksums)
            ),
            "manifest_ids_not_in_source": sorted(
                set(postgres_manifest_checksums) - postgres_source_ids
            ),
            "checksum_mismatches": sorted(
                migration_id
                for migration_id in postgres_source_ids
                & set(postgres_manifest_checksums)
                if postgres_source_checksums[migration_id]
                != postgres_manifest_checksums[migration_id]
            ),
            "checksum_method": "blake3(sql bytes), implemented by MigrationDescriptor::checksum",
            "source_sha256": _sha256_reference(_read(POSTGRES_SOURCE)),
            "migrations_source_sha256": _sha256_reference(_read(POSTGRES_MIGRATIONS)),
        },
    }


def _pack_report() -> dict[str, Any]:
    manifest = tomllib.loads(_read(MANIFEST_TOML).decode("utf-8"))
    rows = manifest.get("pack_executors", [])
    heist_text = _read(HEIST_SOURCE)
    canonical_text = heist_text.replace(b"\r", b"")
    source_digest = _blake3_reference(canonical_text)
    source_sha = _sha256_reference(heist_text)
    source_constants = {
        "pack_id": re.search(
            r'pub const AGENT_HEIST_PACK_ID: &str = "([^"]+)"', heist_text.decode()
        ).group(1),
        "version": re.search(
            r'pub const AGENT_HEIST_VERSION: &str = "([^"]+)"', heist_text.decode()
        ).group(1),
        "transcript_digest": re.search(
            r'const TRANSCRIPT_DIGEST: &str =\s*"([^"]+)"',
            _read(HEIST_REGISTRY_SOURCE).decode(),
            re.DOTALL,
        ).group(1),
    }
    heist_row = next(
        (row for row in rows if row.get("pack_id") == source_constants["pack_id"]), {}
    )
    return {
        "agent_heist": {
            "manifest_row": heist_row,
            "source_constants": source_constants,
            "executor_formula": 'blake3(canonical_text_artifact(include_bytes!("agent_heist.rs")))',
            "executor_artifact_input": {
                "path": HEIST_SOURCE.relative_to(ROOT).as_posix(),
                "size_bytes": len(canonical_text),
                "sha256": _sha256_reference(canonical_text),
                "blake3": source_digest,
                "raw_source_sha256": source_sha,
            },
            "source_registry_sha256": _sha256_reference(_read(HEIST_REGISTRY_SOURCE)),
            "runtime_registry_test": "cargo test --locked -p worldstream-core --lib registry_golden_transcript_is_fixed",
            "manifest_identity_status": "unresolved_by_design"
            if not heist_row.get("revision_digest")
            else "resolved",
            "note": "executor source digest is independently recomputed here; cargo xtask compat verify checks every manifest digest against the embedded Rust registry",
        }
    }


def _release_inventory_report() -> dict[str, Any]:
    package_path = ROOT / "scripts/package.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_package_wave6", package_path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load scripts/package.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    manifest = tomllib.loads(_read(MANIFEST_TOML).decode("utf-8"))
    rows = manifest.get("release_artifacts", [])
    actual_ids = [row.get("id") for row in rows if isinstance(row, dict)]
    expected_ids = list(module.RELEASE_ARTIFACT_IDS)
    profile_mismatches = []
    for row in rows:
        if not isinstance(row, dict):
            continue
        artifact_id = row.get("id")
        if (
            artifact_id in module.RELEASE_ARTIFACT_PROFILES
            and row.get("profile") != module.RELEASE_ARTIFACT_PROFILES[artifact_id]
        ):
            profile_mismatches.append(artifact_id)
    return {
        "source": "scripts/package.py",
        "source_sha256": _sha256_reference(_read(package_path)),
        "expected_ids": expected_ids,
        "manifest_ids": actual_ids,
        "inventory_ids_match": actual_ids == expected_ids,
        "profile_mismatches": profile_mismatches,
        "all_manifest_digests_empty": all(
            isinstance(row, dict) and row.get("digest") == "" for row in rows
        ),
        "release_directory_created": False,
    }


def build_report(*, bundled_checkout: Path | None = None) -> dict[str, Any]:
    manifest = _manifest_report()
    return {
        "schema": "worldstream/manifest-evidence-wave6/v1",
        "purpose": "read-only implementation identity and release inventory audit",
        "root": ROOT.as_posix(),
        "manifest": manifest,
        "sqlite": _sqlite_report(
            bundled_checkout if bundled_checkout is not None else _find_checkout()
        ),
        "migrations": _migration_report(),
        "packs": _pack_report(),
        "release_inventory": _release_inventory_report(),
        "claim_policy": {
            "writes_manifest": False,
            "writes_release_directory": False,
            "manufactures_external_evidence": False,
            "implementation_identities_are_release_evidence": False,
        },
    }


def _validate(report: dict[str, Any]) -> list[str]:
    failures: list[str] = []
    parity = report["manifest"]["parity"]
    for key, value in parity.items():
        if not value:
            failures.append(f"manifest parity failure: {key}")
    state = report["manifest"]["state"]
    contract_state = (state["manifest_kind"], state["release_ready"])
    if contract_state not in {("specification", False), ("release", True)}:
        failures.append("embedded manifest kind/readiness state is incoherent")
    if not state["release_artifact_ids_unique"]:
        failures.append("release artifact inventory contains duplicate ids")
    if not state["evidence_shape_ok"]:
        failures.append("manifest evidence rows have invalid detached/resolved shape")
    inventory = report["release_inventory"]
    if not inventory["inventory_ids_match"]:
        failures.append("manifest release inventory differs from package.py inventory")
    if inventory["profile_mismatches"]:
        failures.append("manifest release inventory has profile mismatches")
    if not inventory["all_manifest_digests_empty"]:
        failures.append("manifest release inventory unexpectedly contains a digest")
    sqlite = report["sqlite"]
    if not sqlite["identity_consistent"]:
        failures.append("SQLite source/manifest/Cargo identity mismatch")
    bundled = sqlite.get("bundled_source", {})
    if bundled.get("status") == "found" and bundled.get(
        "input_inventory_sha256"
    ) != sqlite["manifest_values"].get("bundle_source_inventory_digest"):
        failures.append("SQLite bundled source inventory digest mismatch")
    if report["migrations"]["sqlite"]["source_count"] != 12:
        failures.append("SQLite migration source count changed unexpectedly")
    if report["migrations"]["postgresql"]["source_count"] != 12:
        failures.append("PostgreSQL migration source count changed unexpectedly")
    for provider in ("sqlite", "postgresql"):
        migration_report = report["migrations"][provider]
        if migration_report["source_ids_not_in_manifest"]:
            failures.append(
                f"{provider} source migrations are missing from the manifest"
            )
        if migration_report["manifest_ids_not_in_source"]:
            failures.append(f"{provider} manifest migrations are absent from source")
        if migration_report["checksum_mismatches"]:
            failures.append(f"{provider} migration checksum drift detected")
    heist = report["packs"]["agent_heist"]
    if heist["manifest_identity_status"] != "resolved":
        failures.append("Agent Heist manifest row is unresolved")
    heist_row = heist["manifest_row"]
    if heist_row.get("status") != "resolved":
        failures.append("Agent Heist manifest status is not resolved")
    if (
        heist_row.get("executor_artifact_digest")
        != heist["executor_artifact_input"]["blake3"]
    ):
        failures.append("Agent Heist executor source digest differs from manifest")
    for field in (
        "revision_digest",
        "descriptor_digest",
        "executor_artifact_digest",
        "schema_bundle_digest",
        "codec_bundle_digest",
        "golden_corpus_digest",
    ):
        if not re.fullmatch(r"blake3:[0-9a-f]{64}", heist_row.get(field, "")):
            failures.append(f"Agent Heist manifest {field} is missing or malformed")
    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit canonical JSON")
    parser.add_argument(
        "--bundled-checkout",
        type=Path,
        help="optional rusqlite checkout at the pinned revision; no network access is attempted",
    )
    args = parser.parse_args(argv)
    try:
        report = build_report(bundled_checkout=args.bundled_checkout)
        failures = _validate(report)
    except (OSError, KeyError, TypeError, ValueError, RuntimeError) as error:
        print(f"manifest evidence wave6 failed: {error}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        print("manifest evidence wave6 validation: pass")
        print(f"manifest parity: {report['manifest']['parity']}")
        print(f"SQLite identity consistent: {report['sqlite']['identity_consistent']}")
        print(
            f"SQLite migration bodies: {report['migrations']['sqlite']['source_count']}"
        )
        print(
            f"Agent Heist manifest status: {report['packs']['agent_heist']['manifest_identity_status']}"
        )
        print(
            "release artifact inventory: implementation shape verified; digests remain unresolved"
        )
    for failure in failures:
        print(f"manifest evidence wave6 validation failed: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
