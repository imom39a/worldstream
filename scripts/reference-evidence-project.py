#!/usr/bin/env python3
"""Project verified package acceptance and soak bytes into five reference inputs."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
REFERENCE_PRODUCER_PATH = ROOT / "scripts/release-evidence-produce-reference.py"
FAILURE_PRODUCER_PATH = ROOT / "scripts/release-evidence-produce-failure-soak.py"
MAX_REPORT_BYTES = 16 * 1024 * 1024
SHA256_PREFIX = "sha256:"
KINDS = ("counter", "heist", "sqlite", "postgres", "soak")
EXPECTED_NORMALIZED_ENGINES = {
    "sqlite": {
        "version": "3.53.4",
        "settings": {"journal_mode": "wal", "synchronous": "full"},
        "connection_mode": "embedded",
    },
    "postgresql": {
        "version": "17.11",
        "settings": {
            "server_version_num": "170011",
            "synchronous_commit": "on",
            "transaction_isolation": "read committed",
        },
        "connection_mode": "direct_and_transaction_pooler",
    },
}


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


REFERENCE_PRODUCER = load_module(
    "worldstream_reference_project_release_producer", REFERENCE_PRODUCER_PATH
)
FAILURE_PRODUCER = load_module(
    "worldstream_reference_project_failure_producer", FAILURE_PRODUCER_PATH
)
AGGREGATOR = REFERENCE_PRODUCER.AGGREGATOR


class ProjectionError(RuntimeError):
    """Raw evidence cannot honestly support the normalized reference reports."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProjectionError(message)


def regular_file(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ProjectionError(f"{label} is unavailable") from error
    require(
        stat.S_ISREG(metadata.st_mode)
        and not stat.S_ISLNK(metadata.st_mode)
        and 0 < metadata.st_size <= MAX_REPORT_BYTES,
        f"{label} must be a bounded regular file",
    )
    return path


def read_json(path: Path, label: str) -> tuple[dict[str, Any], bytes]:
    regular_file(path, label)
    raw = path.read_bytes()
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProjectionError(f"{label} is not UTF-8 JSON") from error
    require(isinstance(value, dict), f"{label} must be a JSON object")
    return value, raw


def sha256_bytes(value: bytes) -> str:
    return SHA256_PREFIX + hashlib.sha256(value).hexdigest()


def atomic_write(path: Path, value: object) -> None:
    require(not path.is_symlink() and not path.is_dir(), "unsafe reference output path")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def certified_environment(
    soak: dict[str, Any], acceptance: dict[str, Any]
) -> dict[str, Any]:
    observed = soak.get("environment_observation")
    require(isinstance(observed, dict), "soak host environment was not observed")
    platform_value = observed.get("platform")
    hardware = observed.get("hardware")
    filesystem = observed.get("filesystem")
    observed_engines = observed.get("engines")
    require(
        isinstance(platform_value, dict)
        and set(platform_value)
        == {"system", "distribution", "distribution_version", "machine"}
        and isinstance(hardware, dict)
        and set(hardware) == {"cpu_model", "logical_cpu_count", "memory_bytes"}
        and isinstance(filesystem, dict)
        and set(filesystem) == {"type", "mount_options", "storage_class"}
        and isinstance(observed_engines, dict)
        and observed_engines.get("postgresql")
        == {"status": "not_observed_by_sqlite_process_soak"},
        "soak host/environment observation is incomplete",
    )
    sqlite_observed = observed_engines.get("sqlite")
    require(
        isinstance(sqlite_observed, dict)
        and sqlite_observed.get("status") == "observed_runtime_verified"
        and sqlite_observed.get("profile") == "sqlite-bundled"
        and isinstance(sqlite_observed.get("exact_identity"), str)
        and sqlite_observed["exact_identity"].startswith("sqlite/3.53.4;"),
        "soak did not observe the exact bundled SQLite identity",
    )
    acceptance_environment = acceptance.get("reference_environment")
    acceptance_platform = (
        acceptance_environment.get("platform")
        if isinstance(acceptance_environment, dict)
        else None
    )
    engines = (
        acceptance_environment.get("engines")
        if isinstance(acceptance_environment, dict)
        else None
    )
    require(
        isinstance(acceptance_platform, dict)
        and acceptance_platform.get("system") == platform_value.get("system")
        and acceptance_platform.get("machine") == platform_value.get("machine")
        and isinstance(engines, dict)
        and set(engines) == {"sqlite", "postgresql", "pgbouncer"}
        and {name: engines[name] for name in ("sqlite", "postgresql")}
        == EXPECTED_NORMALIZED_ENGINES,
        "packaged acceptance and soak host/engine observations disagree",
    )
    environment = {
        "platform": platform_value,
        "hardware": hardware,
        "filesystem": filesystem,
        "engines": {
            "sqlite": engines["sqlite"],
            "postgresql": engines["postgresql"],
        },
    }
    try:
        AGGREGATOR._validate_reference_environment(
            "projected", {"reference_environment": environment}
        )
    except AGGREGATOR.EvidenceError as error:
        raise ProjectionError(f"reference host is not certified: {error}") from error
    return environment


def workload(acceptance: dict[str, Any], story: str, backend: str) -> dict[str, Any]:
    try:
        value = acceptance["reference_workloads"][story][backend]["disclosure"]
    except (KeyError, TypeError) as error:
        raise ProjectionError(
            f"{story}.{backend} raw workload disclosure is missing"
        ) from error
    try:
        AGGREGATOR._validate_reference_workload(
            f"{story}.{backend}", {"reference_workload": value}
        )
    except AGGREGATOR.EvidenceError as error:
        raise ProjectionError(
            f"{story}.{backend} raw workload disclosure is invalid: {error}"
        ) from error
    return value


def cell(acceptance: dict[str, Any], story: str, backend: str) -> dict[str, Any]:
    try:
        value = acceptance["cells"][story][backend]
    except (KeyError, TypeError) as error:
        raise ProjectionError(f"{story}.{backend} accepted cell is missing") from error
    require(
        isinstance(value, dict)
        and value.get("exit_code") == 0
        and isinstance(value.get("performance"), dict),
        f"{story}.{backend} accepted cell is not complete",
    )
    return value


def report(
    *,
    schema: str,
    status: str,
    identity: dict[str, Any],
    environment: dict[str, Any],
    workload_value: dict[str, Any],
    measurements: dict[str, Any],
    source: str,
    one_hour: bool = False,
) -> dict[str, Any]:
    value = {
        "schema": schema,
        "status": status,
        "release_evidence": False,
        "performance_class": "reference_non_release",
        "identity": identity,
        "reference_environment": environment,
        "reference_workload": workload_value,
        "measurements": measurements,
        "projection_source": source,
    }
    if one_hour:
        value["one_hour_window_completed"] = True
    return value


def project(args: argparse.Namespace) -> dict[str, Path]:
    manifest = REFERENCE_PRODUCER.ADAPTER.COLLECTOR.load_manifest(
        args.manifest_toml, args.manifest_json
    )
    distribution, _package_report, _package_raw, package_binding = (
        REFERENCE_PRODUCER.verify_packaged_distribution(
            args.package_archive,
            args.package_report,
            args.daemon_bin,
            args.manifest_toml,
            args.manifest_json,
            manifest,
        )
    )
    acceptance, acceptance_raw, acceptance_sha256 = (
        REFERENCE_PRODUCER.verify_packaged_acceptance(
            args.packaged_acceptance_report, distribution, package_binding
        )
    )
    soak, _soak_raw = read_json(args.soak_report, "one-hour process soak report")
    try:
        soak_validated = FAILURE_PRODUCER.validate_soak(
            soak,
            version=manifest["release_candidate"],
            manifest_json_sha256=distribution["manifest_json_sha256"],
            manifest_toml_sha256=distribution["manifest_toml_sha256"],
            packaged_acceptance_sha256=acceptance_sha256,
        )
    except FAILURE_PRODUCER.EvidenceError as error:
        raise ProjectionError(
            f"one-hour process soak is incomplete: {error}"
        ) from error
    reported_distribution = soak_validated["distribution"]
    require(
        reported_distribution
        == {field: distribution[field] for field in reported_distribution},
        "one-hour soak does not bind the independently verified package bytes",
    )
    environment = certified_environment(soak, acceptance)
    identity = {
        "product": "worldstream",
        "profile": "linux-reference",
        "version": manifest["release_candidate"],
        "artifact_sha256": distribution["archive_sha256"],
        "packaged_acceptance_sha256": acceptance_sha256,
    }
    counter_cell = cell(acceptance, "counter", "sqlite")
    heist_cell = cell(acceptance, "heist", "sqlite")
    postgres_cell = cell(acceptance, "counter", "postgres_direct")
    soak_measurements = soak.get("measurements")
    require(
        isinstance(soak_measurements, dict),
        "one-hour soak measurements are missing",
    )
    postgres_performance = postgres_cell["performance"]
    postgres_growth = postgres_performance.get("database_growth")
    require(
        isinstance(postgres_growth, dict),
        "counter.postgres_direct database/WAL measurements are missing",
    )
    projected = {
        "counter": report(
            schema="worldstream/imo-55-live-counter-luna/v1",
            status="completed",
            identity=identity,
            environment=environment,
            workload_value=workload(acceptance, "counter", "sqlite"),
            measurements={
                name: counter_cell["performance"][name]
                for name in ("latency_ms", "load", "fan_out")
            },
            source="packaged-acceptance:counter.sqlite",
        ),
        "heist": report(
            schema="worldstream/imo-57-absent-broker-live/v1",
            status="completed",
            identity=identity,
            environment=environment,
            workload_value=workload(acceptance, "heist", "sqlite"),
            measurements={
                name: heist_cell["performance"][name]
                for name in ("latency_ms", "load", "fan_out")
            },
            source="packaged-acceptance:heist.sqlite",
        ),
        "sqlite": report(
            schema="worldstream/soak-evidence/v1",
            status="pass",
            identity=identity,
            environment=environment,
            workload_value=soak["reference_workload"],
            measurements={
                name: soak_measurements[name]
                for name in (
                    "latency_ms",
                    "memory",
                    "database_growth",
                    "recovery",
                )
            },
            source="one-hour-process-soak:sqlite",
        ),
        "postgres": report(
            schema="worldstream/postgresql-live-evidence/v1",
            status="pass",
            identity=identity,
            environment=environment,
            workload_value=workload(acceptance, "counter", "postgres_direct"),
            measurements={
                "latency_ms": postgres_performance["latency_ms"],
                "memory": postgres_performance["memory"],
                "database_growth": {
                    "status": "measured",
                    "growth_bytes": postgres_growth["growth_bytes"],
                    "wal_growth_bytes": postgres_growth["cluster_wal_growth_bytes"],
                },
                "recovery": postgres_performance["recovery"],
            },
            source="packaged-acceptance:counter.postgres_direct",
        ),
        "soak": report(
            schema="worldstream/soak-evidence/v1",
            status="pass",
            identity=identity,
            environment=environment,
            workload_value=soak["reference_workload"],
            measurements={
                name: soak_measurements[name]
                for name in ("latency_ms", "memory", "database_growth")
            },
            source="one-hour-process-soak:3600s",
            one_hour=True,
        ),
    }
    args.output_dir.mkdir(parents=True, exist_ok=True)
    paths = {kind: args.output_dir / f"{kind}.json" for kind in KINDS}
    input_paths = {
        args.package_archive.resolve(),
        args.package_report.resolve(),
        args.daemon_bin.resolve(),
        args.packaged_acceptance_report.resolve(),
        args.soak_report.resolve(),
        args.manifest_toml.resolve(),
        args.manifest_json.resolve(),
    }
    require(
        not input_paths.intersection(path.resolve() for path in paths.values())
        and args.aggregate_report.resolve() not in input_paths,
        "reference projection outputs must not overwrite raw evidence inputs",
    )
    for kind in KINDS:
        atomic_write(paths[kind], projected[kind])
    aggregate = REFERENCE_PRODUCER.recompute_report(paths)
    REFERENCE_PRODUCER.validate_report(
        aggregate,
        manifest,
        distribution["archive_sha256"],
        acceptance_sha256,
    )
    atomic_write(args.aggregate_report, aggregate)
    require(
        sha256_bytes(acceptance_raw) == acceptance_sha256,
        "packaged acceptance byte identity changed during projection",
    )
    return paths


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--packaged-acceptance-report", type=Path, required=True)
    command.add_argument("--soak-report", type=Path, required=True)
    command.add_argument("--package-archive", type=Path, required=True)
    command.add_argument("--package-report", type=Path, required=True)
    command.add_argument("--daemon-bin", type=Path, required=True)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument("--aggregate-report", type=Path, required=True)
    command.add_argument(
        "--manifest-toml",
        type=Path,
        default=REFERENCE_PRODUCER.ADAPTER.COLLECTOR.DEFAULT_MANIFEST_TOML,
    )
    command.add_argument(
        "--manifest-json",
        type=Path,
        default=REFERENCE_PRODUCER.ADAPTER.COLLECTOR.DEFAULT_MANIFEST_JSON,
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        paths = project(args)
    except (
        ProjectionError,
        REFERENCE_PRODUCER.ReferenceError,
        REFERENCE_PRODUCER.ADAPTER.COLLECTOR.CollectionError,
        OSError,
        KeyError,
        TypeError,
    ) as error:
        print(f"reference projection failed: {error}", file=sys.stderr)
        return 1
    print(
        "wrote verified reference projection: "
        + ",".join(str(paths[kind]) for kind in KINDS)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
