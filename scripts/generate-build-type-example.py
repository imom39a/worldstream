#!/usr/bin/env python3
"""Regenerate a canonical illustrative statement for a release build type."""

from __future__ import annotations

import argparse
import importlib.util
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"


def load_identity():
    name = "worldstream_build_type_example_generator_identity"
    spec = importlib.util.spec_from_file_location(name, IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def atomic_replace(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink() or (path.exists() and not path.is_file()):
        raise RuntimeError(f"unsafe build-type example output: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--release-inventory",
        choices=(
            "worldstream/release-inventory/runtime-packs-studio-v1",
            "worldstream/release-inventory/cli-first-v1",
        ),
        help=(
            "omission regenerates the historical v4 example; select the "
            "CLI-first inventory for the active v5 example"
        ),
    )
    args = parser.parse_args()
    identity = load_identity()
    try:
        entries = identity.source_entries_from_root(ROOT)
        if args.release_inventory == identity.INVENTORY.CLI_FIRST.identity:
            for relative in identity.CLI_FIRST_AGGREGATION_MATERIAL_PATHS:
                entries[relative] = identity.regular_bytes(
                    ROOT / relative, f"source material {relative}"
                )
            value = identity.build_type_v5_example(entries)
            identity.validate_build_type_v5_example(value, entries)
            default_output = ROOT / identity.CLI_FIRST_BUILD_TYPE_EXAMPLE_PATH
            version = 5
        else:
            value = identity.build_type_v4_example(entries)
            identity.validate_build_type_v4_example(value, entries)
            default_output = ROOT / identity.BUILD_TYPE_EXAMPLE_PATH
            version = 4
        content = identity.canonical_json(value)
        destination = args.output or default_output
        atomic_replace(destination, content)
    except (identity.IdentityError, OSError, RuntimeError) as error:
        print(f"build-type example generation failed: {error}", file=sys.stderr)
        return 1
    print(f"generated canonical v{version} build-type example: {destination}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
