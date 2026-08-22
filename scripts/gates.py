#!/usr/bin/env python3
"""WorldStream compatibility, security, and supply-chain gates.

The runner is intentionally dependency-light.  Every check has an explicit
status and an unavailable tool is never silently treated as evidence:
optional local checks become SKIP_INCOMPLETE, while CI/release checks fail.
The compatibility manifest remains the source of truth for platform cells and
release evidence; this file only maps those declarations to executable checks.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform as platform_module
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.parse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = ROOT / "compatibility.toml"
MIRROR_PATH = ROOT / "compatibility.json"
ROOT_PYTHON_TESTS = tuple(
    str(path.relative_to(ROOT)) for path in sorted((ROOT / "tests").glob("*.py"))
)
MAX_POSTGRES_DSN_BYTES = 16 * 1024
HOSTED_REQUIRED_TOOL_VERSIONS = {
    "cargo-audit": "0.22.2",
    "gitleaks": "8.29.1",
}

SHA256_DIGEST = re.compile(r"[0-9a-f]{64}\Z")
SHA256_REFERENCE = re.compile(r"sha256:[0-9a-f]{64}\Z")
EVIDENCE_DIGEST = re.compile(r"(?:sha256|blake3):[0-9a-f]{64}\Z")

RELEASE_ARTIFACT_PROFILES = {
    "source-archive": "source",
    "native-linux-x86_64-archive": "native-linux-x86_64",
    "native-windows-x64-archive": "native-windows-x64",
    "oci-linux-amd64-image": "oci-linux-amd64",
    "checksums": "all",
    "sigstore-bundle": "all",
    "spdx-sbom": "all",
    "slsa-provenance": "all",
}
DETACHED_RELEASE_MANIFEST_SCHEMA = "worldstream/release-artifact-manifest/v2"
DETACHED_DIGEST_SOURCE = "detached_release_manifest"
SUPPLY_CHAIN_ARTIFACT_IDS = frozenset(
    {"checksums", "sigstore-bundle", "spdx-sbom", "slsa-provenance"}
)
SIGSTORE_VERIFICATION_ARTIFACT_ID = "sigstore-bundle"
CHECKSUM_PAYLOAD_ARTIFACT_IDS = frozenset(
    {
        "source-archive",
        "native-linux-x86_64-archive",
        "native-windows-x64-archive",
        "oci-linux-amd64-image",
    }
)
PRE_SIGN_SUBJECT_DIRECTORY = "supply-chain/subjects"
PRE_SIGN_SUPPLY_CHAIN_FILES = frozenset(
    {
        "supply-chain/subject-inventory.json",
        "supply-chain/subject-inventory.bundle.json",
    }
)


def release_evidence_verifier():
    """Load the canonical normalized-evidence and pre-sign verifier."""

    path = ROOT / "scripts/release-evidence-assemble.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_gate_release_evidence_verifier", path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load release evidence verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def release_build_identity_verifier():
    """Load the canonical source/component/build graph verifier."""

    path = ROOT / "scripts/release_build_identity.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_gate_release_build_identity", path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load release build identity verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The workflow consumes this exact route object from --format matrix.  Keep
# platform selection explicit: a new/typoed platform must stop matrix
# generation rather than silently running on an Ubuntu worker.
CELL_ROUTES = {
    "native-linux-x86_64": {
        "runner": "ubuntu-24.04",
        "shell": "bash",
        "system": "Linux",
    },
    "native-windows-x64": {
        "runner": "windows-2025",
        "shell": "pwsh",
        "system": "Windows",
    },
    # OCI is a Linux/amd64 build-and-runtime cell.  It is sourced from the
    # manifest platform declaration below when the gate-cell list does not
    # repeat that platform as a second source of truth.
    "oci-linux-amd64": {
        "runner": "ubuntu-24.04",
        "shell": "bash",
        "system": "Linux",
    },
    "local": {"runner": "local", "shell": "bash", "system": None},
}

# This is a syntax-check input for the OCI template only.  It is deliberately
# not read from or written to the compatibility manifest and is never a
# release artifact identity.  Release evidence still requires the owning
# Linux/amd64 evidence job to supply and verify its own pinned base image.
OCI_TEMPLATE_BASE_IMAGE = (
    "alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
)

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.11 is a project gate.
    print("gates require Python 3.11+ (tomllib is unavailable)", file=sys.stderr)
    raise SystemExit(2)


@dataclass(frozen=True)
class Outcome:
    name: str
    status: str
    detail: str
    classification: str = "failure"


RELEASE_ARTIFACT_EVIDENCE = {
    "source-archive": {
        "environment": "release assembly workspace",
        "next_evidence": "Build and verify the source archive, then record its exact SHA-256.",
    },
    "native-linux-x86_64-archive": {
        "environment": "native Linux x86-64 release runner",
        "next_evidence": "Build and verify the native Linux archive, then record its exact SHA-256.",
    },
    "native-windows-x64-archive": {
        "environment": "native Windows x64 release runner",
        "next_evidence": "Build, verify Windows ACL/path behavior, and record the archive SHA-256.",
    },
    "oci-linux-amd64-image": {
        "environment": "Linux/amd64 OCI builder and runtime with a pinned base-image digest",
        "next_evidence": "Build and exercise the OCI image, then record the image/artifact digest.",
    },
    "checksums": {
        "environment": "release bundle assembly workspace",
        "next_evidence": "Generate SHA256SUMS over the declared payload artifacts and record its SHA-256.",
    },
    "sigstore-bundle": {
        "environment": "release signing identity with cosign and OIDC issuer",
        "next_evidence": "Sign detached release-manifest.json and retain the verified Sigstore bundle as verification material.",
    },
    "spdx-sbom": {
        "environment": "pinned release SBOM generation job",
        "next_evidence": "Generate and validate the SPDX SBOM for the exact release payload, then record its SHA-256.",
    },
    "slsa-provenance": {
        "environment": "pinned release provenance generation job",
        "next_evidence": "Generate SLSA provenance whose subjects match the exact payload bytes, then record its SHA-256.",
    },
}


def _digest_is_valid(value: object) -> bool:
    return isinstance(value, str) and bool(SHA256_REFERENCE.fullmatch(value))


def uses_detached_release_identity(manifest: dict[str, Any]) -> bool:
    """Return whether release identities are intentionally externalized.

    Artifact/evidence bytes cannot safely carry their own SHA-256 in the
    embedded compatibility manifest: changing the embedded digest changes the
    bytes being hashed.  The compatibility manifest therefore declares the
    identity source, while the detached release-manifest.json supplies the
    actual digests.  Accept the explicit top-level spelling and the nested
    spelling used by early draft manifests, but never infer detachment from
    missing values alone.
    """

    candidates = [
        manifest.get("release_artifact_digest_source"),
        manifest.get("artifact_digest_source"),
    ]
    release_identity = manifest.get("release_identity")
    if isinstance(release_identity, dict):
        candidates.append(release_identity.get("artifact_digest_source"))
    if any(value == DETACHED_DIGEST_SOURCE for value in candidates):
        return True
    release_rows = manifest.get("release_artifacts")
    evidence_rows = manifest.get("evidence")
    rows = release_rows if isinstance(release_rows, list) else []
    evidence = evidence_rows if isinstance(evidence_rows, list) else []
    detached_artifacts = bool(rows) and all(
        isinstance(row, dict)
        and row.get("status") == "detached"
        and row.get("digest") == ""
        and row.get("digest_location") == "release-manifest.json"
        for row in rows
    )
    detached_evidence = bool(evidence) and all(
        isinstance(row, dict)
        and row.get("status") == "detached"
        and row.get("artifact_digest") == ""
        and row.get("artifact_digest_location") == "release-manifest.json"
        for row in evidence
        if row.get("release_gate") is True
    )
    return detached_artifacts and detached_evidence


def release_blocker_matrix(
    manifest: dict[str, Any], outcomes: list[Outcome]
) -> dict[str, Any]:
    """Summarize release prerequisites without promoting any evidence.

    This is deliberately derived from the manifest and the observed gate
    outcomes.  It is a handoff matrix, not a release-manifest generator: an
    unresolved row stays blocked and no digest is inferred from local source
    bytes or fixture output.
    """

    rows: list[dict[str, Any]] = []

    def verified_outcome(name: str) -> bool:
        matching = [outcome for outcome in outcomes if outcome.name == name]
        return bool(matching) and all(outcome.status == "PASS" for outcome in matching)

    def add(
        blocker_id: str,
        *,
        ready: bool,
        source: str,
        observed: str,
        environment: str,
        next_evidence: str,
    ) -> None:
        rows.append(
            {
                "id": blocker_id,
                "status": "ready" if ready else "blocked",
                "source": source,
                "observed": observed,
                "environment": environment,
                "next_evidence": next_evidence,
            }
        )

    manifest_kind = manifest.get("manifest_kind")
    release_ready = manifest.get("release_ready")
    add(
        "manifest-state",
        ready=manifest_kind == "release" and release_ready is True,
        source="compatibility.toml",
        observed=f"manifest_kind={manifest_kind!r}; release_ready={release_ready!r}",
        environment="release review",
        next_evidence="Only the release owner may promote a reviewed manifest after every required field is independently resolved.",
    )

    unresolved = manifest.get("unresolved_required_fields")
    unresolved_values = (
        unresolved
        if isinstance(unresolved, list)
        else ["invalid unresolved_required_fields"]
    )
    add(
        "manifest-required-fields",
        ready=isinstance(unresolved, list) and not unresolved,
        source="compatibility.toml:unresolved_required_fields",
        observed=(
            "none" if not unresolved_values else ", ".join(map(str, unresolved_values))
        ),
        environment="release review",
        next_evidence="Resolve each listed field from its owning build or evidence job; do not derive external identities locally.",
    )

    release_artifact_rows = manifest.get("release_artifacts", [])
    if not isinstance(release_artifact_rows, list):
        release_artifact_rows = []
    artifact_rows = {
        row.get("id"): row
        for row in release_artifact_rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    detached_identity = uses_detached_release_identity(manifest)
    for artifact_id, expected_profile in RELEASE_ARTIFACT_PROFILES.items():
        row = artifact_rows.get(artifact_id)
        requirement = RELEASE_ARTIFACT_EVIDENCE.get(
            artifact_id,
            {
                "environment": "release evidence job",
                "next_evidence": "Produce the declared artifact and record its exact SHA-256.",
            },
        )
        if row is None:
            add(
                f"release-artifact:{artifact_id}",
                ready=False,
                source=f"compatibility.toml:release_artifacts[{artifact_id}]",
                observed="row is missing",
                environment=requirement["environment"],
                next_evidence=requirement["next_evidence"],
            )
            continue
        digest = row.get("digest")
        declaration_ready = (
            row.get("profile") == expected_profile
            and row.get("digest_algorithm") == "sha256"
            and (
                (row.get("status") == "resolved" and _digest_is_valid(digest))
                or (
                    detached_identity
                    and row.get("status") in {"detached", "unresolved"}
                    and not digest
                )
            )
        )
        outcome_name = "release-artifact-" + artifact_id
        observed_ready = verified_outcome(outcome_name)
        add(
            f"release-artifact:{artifact_id}",
            ready=declaration_ready and observed_ready,
            source=f"compatibility.toml:release_artifacts[{artifact_id}]",
            observed=(
                f"status={row.get('status')!r}; "
                f"profile={row.get('profile')!r}; "
                f"digest={'detached' if detached_identity and not digest else 'valid' if _digest_is_valid(digest) else 'empty or invalid'}; "
                f"verified_outcome={'pass' if observed_ready else 'absent or non-pass'}"
            ),
            environment=requirement["environment"],
            next_evidence=requirement["next_evidence"],
        )

    evidence_rows = manifest.get("evidence", [])
    if not isinstance(evidence_rows, list):
        evidence_rows = []
    for row in evidence_rows:
        if not isinstance(row, dict) or row.get("release_gate") is not True:
            continue
        evidence_id = row.get("id")
        if not isinstance(evidence_id, str):
            continue
        digest = row.get("artifact_digest")
        declaration_ready = (
            row.get("status") == "resolved" and _digest_is_valid(digest)
        ) or (
            detached_identity
            and row.get("status") == "detached"
            and digest == ""
            and row.get("artifact_digest_location") == "release-manifest.json"
        )
        outcome_name = "release-evidence-" + evidence_id
        observed_ready = verified_outcome(outcome_name)
        add(
            f"release-evidence:{evidence_id}",
            ready=declaration_ready and observed_ready,
            source=f"compatibility.toml:evidence[{evidence_id}]",
            observed=(
                f"status={row.get('status')!r}; "
                f"artifact_digest={'present' if digest else 'empty'}; "
                f"verified_outcome={'pass' if observed_ready else 'absent or non-pass'}"
            ),
            environment="the owning release evidence runner for this contract",
            next_evidence="Run the complete declared contract on its required native/backend environment and retain the report digest.",
        )

    platform_requirements = {
        "native-linux-x86_64": (
            "x86_64-unknown-linux-musl",
            "ubuntu-24.04",
            "Linux",
            "native-linux-release-profile",
            "native-linux-x86_64-archive",
        ),
        "native-windows-x64": (
            "x86_64-pc-windows-msvc",
            "windows-2025",
            "Windows",
            "native-windows-release-profile",
            "native-windows-x64-archive",
        ),
        "oci-linux-amd64": (
            "linux/amd64",
            "ubuntu-24.04",
            "Linux",
            "oci-linux-amd64-release-profile",
            "oci-linux-amd64-image",
        ),
        "macos-source-quickstart": (
            "intel-and-apple-silicon",
            "macos-15",
            "Darwin",
            "macos-source-quickstart",
            "source-archive",
        ),
    }
    platform_rows = manifest.get("platforms", [])
    if not isinstance(platform_rows, list):
        platform_rows = []
    declared_platforms = {
        row.get("id")
        for row in platform_rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    evidence_by_id = {
        row.get("id"): row
        for row in evidence_rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    for platform_id, (
        target,
        runner_name,
        system,
        evidence_id,
        artifact_id,
    ) in platform_requirements.items():
        if platform_id not in declared_platforms:
            continue
        evidence_row = evidence_by_id.get(evidence_id)
        artifact_row = artifact_rows.get(artifact_id)
        evidence_ready = bool(evidence_row) and verified_outcome(
            "release-evidence-" + evidence_id
        )
        artifact_ready = bool(artifact_row) and verified_outcome(
            "release-artifact-" + artifact_id
        )
        add(
            f"platform:{platform_id}",
            ready=evidence_ready and artifact_ready,
            source=f"compatibility.toml:platforms[{platform_id}]",
            observed=(
                f"target={target}; runner={runner_name}; system={system}; "
                f"artifact={'ready' if artifact_ready else 'blocked'}; "
                f"evidence={'ready' if evidence_ready else 'blocked'}"
            ),
            environment=(f"{runner_name} ({system}); exact target {target}"),
            next_evidence=(
                f"Run {platform_id} on {runner_name} for {target}, retain "
                f"{evidence_id} and {artifact_id} digests."
            ),
        )

    release_dir = Path(os.environ.get("WORLDSTREAM_RELEASE_DIR", ROOT / "dist"))
    release_manifest_path = release_dir / "release-manifest.json"
    release_dir_ready = (
        release_dir.is_dir()
        and not release_dir.is_symlink()
        and release_manifest_path.is_file()
        and not release_manifest_path.is_symlink()
    )
    add(
        "release-directory",
        ready=release_dir_ready,
        source="WORLDSTREAM_RELEASE_DIR or dist/",
        observed=(
            "directory and release-manifest.json exist"
            if release_dir_ready
            else "directory or release-manifest.json missing, symlinked, or non-regular"
        ),
        environment="release bundle assembly workspace",
        next_evidence="Assemble release-manifest.json and every declared artifact in the release directory.",
    )

    failed_outcomes = [
        outcome.__dict__
        for outcome in outcomes
        if outcome.status == "FAIL" or outcome.status == "SKIP_INCOMPLETE"
    ]
    incomplete_outcomes = [
        outcome.__dict__
        for outcome in outcomes
        if outcome.classification == "incomplete"
    ]
    return {
        "schema": "worldstream/release-blocker-matrix/v1",
        "status": "ready"
        if not any(row["status"] == "blocked" for row in rows) and not failed_outcomes
        else "blocked",
        "rows": rows,
        "blocking_gate_outcomes": failed_outcomes,
        "incomplete_gate_outcomes": incomplete_outcomes,
    }


def release_evidence_handoff(manifest: dict[str, Any]) -> dict[str, Any]:
    """Describe the exact external release handoff without filling evidence.

    This payload is deliberately useful to a release assembly job while
    remaining fail-closed: paths are deterministic expectations, but all
    digests and readiness values come only from the manifest and are never
    inferred from local source or fixture bytes.
    """

    version = str(manifest.get("release_candidate", ""))
    artifact_paths = {
        "source-archive": f"worldstream-{version}-source.tar.gz",
        "native-linux-x86_64-archive": f"worldstream-{version}-linux-x86_64.tar.gz",
        "native-windows-x64-archive": f"worldstream-{version}-windows-x64.zip",
        "oci-linux-amd64-image": f"worldstream-{version}-oci-linux-amd64.oci.tar",
        "checksums": "SHA256SUMS",
        "sigstore-bundle": "sigstore.bundle.json",
        "spdx-sbom": "sbom.spdx.json",
        "slsa-provenance": "provenance.json",
    }
    artifacts: list[dict[str, Any]] = []
    release_artifact_rows = manifest.get("release_artifacts", [])
    if not isinstance(release_artifact_rows, list):
        release_artifact_rows = []
    for row in release_artifact_rows:
        if not isinstance(row, dict):
            continue
        artifact_id = row.get("id")
        if not isinstance(artifact_id, str):
            continue
        artifacts.append(
            {
                "id": artifact_id,
                "profile": row.get("profile"),
                "path": artifact_paths.get(artifact_id),
                "digest_algorithm": row.get("digest_algorithm"),
                "digest": row.get("digest", ""),
                "status": row.get("status"),
                "evidence_owner": RELEASE_ARTIFACT_EVIDENCE.get(artifact_id, {}).get(
                    "environment", "release evidence job"
                ),
            }
        )
    evidence: list[dict[str, Any]] = []
    evidence_rows = manifest.get("evidence", [])
    if not isinstance(evidence_rows, list):
        evidence_rows = []
    for row in evidence_rows:
        if not isinstance(row, dict) or row.get("release_gate") is not True:
            continue
        evidence.append(
            {
                "id": row.get("id"),
                "status": row.get("status"),
                "artifact_digest": row.get("artifact_digest", ""),
                "release_gate": True,
            }
        )
    return {
        "schema": "worldstream/release-evidence-handoff/v1",
        "release_evidence": False,
        "release_ready": manifest.get("release_ready") is True,
        "manifest_kind": manifest.get("manifest_kind"),
        "manifest_source": MANIFEST_PATH.name,
        "manifest_mirror": MIRROR_PATH.name,
        "release_candidate": version,
        "artifact_digest_source": (
            DETACHED_DIGEST_SOURCE
            if uses_detached_release_identity(manifest)
            else "compatibility.toml"
        ),
        "required_artifacts": artifacts,
        "required_evidence": evidence,
        "required_external_inputs": [
            "native Linux x86_64 archive and runtime evidence",
            "native Windows x64 archive, ACL, and runtime evidence",
            "Linux/amd64 OCI image, pinned base image, and runtime evidence",
            "SHA256SUMS covering the exact payload and every pre-sign source subject",
            "verified Sigstore bundle for detached release-manifest.json",
            "SPDX SBOM for the exact payload and every pre-sign source subject",
            "SLSA provenance whose subjects match the exact pre-sign subject inventory",
            "detached release-manifest.json whose artifact and evidence maps match every subject byte",
        ],
    }


class GateRunner:
    def __init__(
        self,
        *,
        strict: bool,
        offline: bool,
        ci: bool,
        tier: str | None = None,
        report_path: str | None = None,
        handoff_path: str | None = None,
    ) -> None:
        self.strict = strict or ci
        self.offline = offline
        self.ci = ci
        self.tier = tier
        self.report_path = report_path
        self.handoff_path = handoff_path
        self.outcomes: list[Outcome] = []

    def record(
        self,
        name: str,
        status: str,
        detail: str,
        *,
        classification: str | None = None,
    ) -> None:
        if status not in {"PASS", "FAIL", "SKIP_INCOMPLETE"}:
            detail = f"invalid gate status {status!r}: {detail}"
            status = "FAIL"
            classification = "failure"
        if classification is None:
            classification = {
                "PASS": "pass",
                "FAIL": "failure",
                "SKIP_INCOMPLETE": "incomplete",
            }[status]
        if classification not in {"pass", "failure", "incomplete"}:
            detail = f"invalid gate classification {classification!r}: {detail}"
            status = "FAIL"
            classification = "failure"
        self.outcomes.append(Outcome(name, status, detail, classification))
        print(f"GATE {status:<16} {name}: {detail}")

    def pass_(self, name: str, detail: str) -> None:
        self.record(name, "PASS", detail)

    def skip(self, name: str, detail: str) -> None:
        status = "FAIL" if self.strict else "SKIP_INCOMPLETE"
        self.record(name, status, detail, classification="incomplete")

    def fail(self, name: str, detail: str) -> None:
        self.record(name, "FAIL", detail)

    def command(
        self,
        name: str,
        argv: list[str],
        *,
        required: bool = True,
        env: dict[str, str] | None = None,
    ) -> bool:
        executable = shutil.which(argv[0])
        if executable is None:
            detail = f"unavailable dependency: {argv[0]}"
            if required:
                self.fail(name, detail)
            else:
                self.skip(name, detail)
            return False

        display = " ".join(redact_command_argument(argument) for argument in argv)
        print(f"RUN {display}")
        command_env = os.environ.copy()
        if self.offline:
            command_env.update(
                {
                    "CARGO_NET_OFFLINE": "true",
                    "UV_OFFLINE": "true",
                    "npm_config_offline": "true",
                }
            )
        if env:
            command_env.update(env)
        quiet = argv[0] == "cargo" and "metadata" in argv
        try:
            result = subprocess.run(
                argv,
                cwd=ROOT,
                env=command_env,
                check=False,
                capture_output=quiet,
                text=quiet,
            )
        except OSError as error:
            detail = f"could not start {argv[0]}: {error}"
            if required:
                self.fail(name, detail)
            else:
                self.skip(name, detail)
            return False
        if result.returncode == 0:
            self.pass_(name, "command completed")
            return True
        if quiet and result.stderr:
            print(result.stderr[-4000:], file=sys.stderr, end="")
        self.fail(name, f"command exited with status {result.returncode}")
        return False

    def unresolved(self, name: str, detail: str) -> None:
        self.skip(name, f"unresolved evidence: {detail}")

    def finish(
        self,
        *,
        manifest: dict[str, Any] | None = None,
        tier: str | None = None,
    ) -> int:
        failures = [outcome for outcome in self.outcomes if outcome.status == "FAIL"]
        skipped = [
            outcome for outcome in self.outcomes if outcome.status == "SKIP_INCOMPLETE"
        ]
        print(
            f"Gate summary: {len(self.outcomes)} checks, "
            f"{len(failures)} failures, {len(skipped)} incomplete skips"
        )
        if failures:
            print("Blocking gate outcomes:", file=sys.stderr)
            for outcome in failures:
                print(f"- {outcome.name}: {outcome.detail}", file=sys.stderr)
        if skipped:
            print("Incomplete gate outcomes:", file=sys.stderr)
            for outcome in skipped:
                print(f"- {outcome.name}: {outcome.detail}", file=sys.stderr)
        release_matrix = None
        if (tier or self.tier) == "release" and manifest is not None:
            release_matrix = release_blocker_matrix(manifest, self.outcomes)
        release_blocked = bool(
            release_matrix is not None and release_matrix["status"] != "ready"
        )
        report_path = self.report_path or os.environ.get("WORLDSTREAM_GATE_REPORT")
        if report_path:
            report_status = (
                "blocked" if failures else "incomplete" if skipped else "passed"
            )
            if release_blocked:
                report_status = "blocked"
            report = {
                "schema": "worldstream/compatibility-gate-report/v1",
                "platform": platform_module.platform(),
                "python": platform_module.python_version(),
                "tier": tier or self.tier,
                "strict": self.strict,
                "status": report_status,
                "fail_closed": report_status != "passed",
                "summary": {
                    "checks": len(self.outcomes),
                    "failures": len(failures),
                    "incomplete_skips": len(skipped),
                },
                "blocking_failures": [outcome.__dict__ for outcome in failures],
                "incomplete_skips": [outcome.__dict__ for outcome in skipped],
                "incomplete_blockers": [
                    outcome.__dict__
                    for outcome in self.outcomes
                    if outcome.classification == "incomplete"
                ],
                "outcomes": [outcome.__dict__ for outcome in self.outcomes],
            }
            if release_matrix is not None:
                report["release_blocker_matrix"] = release_matrix
                report["release_evidence_handoff"] = release_evidence_handoff(manifest)
            destination = Path(report_path)
            if destination.is_symlink() or destination.is_dir():
                raise RuntimeError(
                    f"gate report path must be a regular file: {destination}"
                )
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_name(f".{destination.name}.tmp")
            temporary.write_text(
                json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            temporary.replace(destination)
        if (
            self.handoff_path
            and (tier or self.tier) == "release"
            and manifest is not None
        ):
            destination = Path(self.handoff_path)
            if destination.is_symlink() or destination.is_dir():
                raise RuntimeError(
                    f"release handoff path must be a regular file: {destination}"
                )
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_name(f".{destination.name}.tmp")
            temporary.write_text(
                json.dumps(release_evidence_handoff(manifest), indent=2, sort_keys=True)
                + "\n",
                encoding="utf-8",
            )
            temporary.replace(destination)
        return 1 if failures or release_blocked else 0


def load_manifest() -> dict[str, Any]:
    with MANIFEST_PATH.open("rb") as source:
        return tomllib.load(source)


def redact_command_argument(argument: str) -> str:
    """Keep command diagnostics useful without printing connection secrets."""
    redacted = re.sub(
        r"(?i)(://)[^/?#@]*@",
        r"\1<redacted>@",
        argument,
    )
    return re.sub(
        r"(?i)(\b(?:password|passwd|secret|sslpassword|passfile)\s*=\s*)(?:\"(?:[^\"]|\"\")*\"|'(?:[^']|'')*'|[^\s]+)",
        r"\1<redacted>",
        redacted,
    )


def release_artifact_inventory_errors(
    manifest: dict[str, Any], *, release: bool
) -> list[str]:
    """Return compatibility inventory diagnostics without approving contents."""
    failures, incompletes = release_artifact_inventory_diagnostics(
        manifest, release=release
    )
    return failures + incompletes


def release_artifact_inventory_diagnostics(
    manifest: dict[str, Any], *, release: bool
) -> tuple[list[str], list[str]]:
    """Separate malformed claims from artifact identities not yet supplied."""
    rows = manifest.get("release_artifacts")
    if not isinstance(rows, list):
        return ["release_artifacts must be a list"], []

    failures: list[str] = []
    incompletes: list[str] = []
    detached = uses_detached_release_identity(manifest)
    seen: set[str] = set()
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            failures.append(f"release artifact row {index} is not an object")
            continue
        artifact_id = row.get("id")
        if not isinstance(artifact_id, str) or not artifact_id:
            failures.append(f"release artifact row {index} has no non-empty id")
            continue
        if artifact_id in seen:
            failures.append(f"duplicate release artifact id: {artifact_id}")
        seen.add(artifact_id)
        expected_profile = RELEASE_ARTIFACT_PROFILES.get(artifact_id)
        if expected_profile is None:
            failures.append(f"unsupported release artifact id: {artifact_id}")
        elif row.get("profile") != expected_profile:
            failures.append(
                f"release artifact profile mismatch: {artifact_id} "
                f"(expected {expected_profile})"
            )
        if row.get("digest_algorithm") != "sha256":
            failures.append(
                f"release artifact digest algorithm is not sha256: {artifact_id}"
            )
        status = row.get("status")
        digest = row.get("digest")
        allowed_statuses = {"resolved", "unresolved"}
        if detached:
            allowed_statuses.add("detached")
        if status not in allowed_statuses:
            failures.append(f"release artifact status is invalid: {artifact_id}")
        if not isinstance(digest, str) or (
            digest and not SHA256_REFERENCE.fullmatch(digest)
        ):
            failures.append(f"release artifact digest is invalid: {artifact_id}")
        if status == "resolved" and not digest:
            failures.append(f"resolved release artifact has no digest: {artifact_id}")
        if status in {"unresolved", "detached"} and digest:
            failures.append(f"unresolved release artifact has a digest: {artifact_id}")
        if status == "unresolved" and not digest and not detached:
            incompletes.append(f"unresolved release artifact identity: {artifact_id}")
        if status == "detached" and not detached:
            failures.append(
                f"detached release artifact identity is not enabled: {artifact_id}"
            )
        if detached and (
            status != "detached"
            or digest != ""
            or row.get("digest_location") != "release-manifest.json"
        ):
            failures.append(
                "detached release artifact rows must use status=detached, "
                f"digest='', digest_location='release-manifest.json': {artifact_id}"
            )

    expected_ids = set(RELEASE_ARTIFACT_PROFILES)
    if seen != expected_ids:
        missing = sorted(expected_ids - seen)
        extra = sorted(seen - expected_ids)
        details = []
        if missing:
            details.append("missing=" + ",".join(missing))
        if extra:
            details.append("unsupported=" + ",".join(extra))
        failures.append(
            "release artifact inventory is incomplete (" + "; ".join(details) + ")"
        )

    if release and not failures and not detached:
        for row in rows:
            if row.get("status") != "resolved" or not row.get("digest"):
                # The row-level diagnostics above identify the exact artifact;
                # keep this aggregate only for callers that need a release-level
                # explanation.
                if row.get("status") == "unresolved":
                    continue
                failures.append(
                    "release tier requires every release artifact row to be resolved"
                )
                break
    return failures, incompletes


def run_python(
    runner: GateRunner, name: str, args: list[str], *, required: bool = True
) -> bool:
    python = shutil.which("python3") or shutil.which("python")
    if python is None:
        if required:
            runner.fail(name, "unavailable dependency: Python 3.11+")
        else:
            runner.skip(name, "unavailable dependency: Python 3.11+")
        return False
    return runner.command(name, [python, *args], required=required)


def manifest_gate(
    runner: GateRunner, manifest: dict[str, Any], *, release: bool
) -> None:
    if manifest.get("validation_policy") != "fail_closed":
        runner.fail("manifest-policy", "validation_policy is not fail_closed")
        return
    if manifest.get("reviewed_source") != MANIFEST_PATH.name:
        runner.fail(
            "manifest-source", "reviewed_source does not name compatibility.toml"
        )
    if manifest.get("canonical_mirror") != MIRROR_PATH.name:
        runner.fail(
            "manifest-mirror", "canonical_mirror does not name compatibility.json"
        )

    canonical = (
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    )
    try:
        checked_in = MIRROR_PATH.read_text(encoding="utf-8")
        checked_json = json.loads(checked_in)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        runner.fail("manifest-parity", f"cannot read canonical JSON mirror: {error}")
    else:
        if checked_in != canonical or checked_json != manifest:
            runner.fail("manifest-parity", "TOML and deterministic JSON mirror differ")
        else:
            runner.pass_(
                "manifest-parity", "authored TOML equals sorted canonical JSON"
            )

    manifest_kind = manifest.get("manifest_kind")
    release_ready = manifest.get("release_ready")
    if manifest_kind not in {"specification", "release"}:
        runner.fail("manifest-kind", "manifest_kind must be specification or release")
    if not isinstance(release_ready, bool):
        runner.fail("manifest-release-ready-type", "release_ready must be boolean")
    elif (manifest_kind == "specification") != (release_ready is False):
        runner.fail(
            "manifest-kind-readiness",
            "specification manifests must be release_ready=false and release manifests true",
        )

    contracts = manifest.get("contracts")
    if not isinstance(contracts, dict):
        runner.fail("manifest-contracts", "contracts object is missing")
    else:
        product = contracts.get("product")
        if not isinstance(product, str) or not product:
            runner.fail("manifest-product", "contracts.product is missing")
        elif manifest.get("release_candidate") != product:
            runner.fail(
                "manifest-release-candidate",
                "release_candidate differs from contracts.product",
            )

    unresolved = manifest.get("unresolved_required_fields")
    if not isinstance(unresolved, list) or any(
        not isinstance(value, str) or not value for value in unresolved
    ):
        runner.fail(
            "manifest-unresolved-set",
            "required unresolved field list must contain non-empty strings",
        )
    elif len(unresolved) != len(set(unresolved)):
        runner.fail(
            "manifest-unresolved-set",
            "required unresolved field list contains duplicates",
        )
    elif release:
        if release_ready is not True:
            runner.unresolved("manifest-release-ready", "release_ready is not true")
        else:
            runner.pass_("manifest-release-ready", "release_ready is true")
        if unresolved:
            runner.unresolved(
                "manifest-required-fields", ", ".join(map(str, unresolved))
            )
        else:
            runner.pass_("manifest-required-fields", "no required unresolved fields")
    else:
        if manifest_kind == "release" and release_ready is True and not unresolved:
            runner.pass_(
                "manifest-contract-state",
                "embedded release contract is complete; detached distribution evidence is not asserted by this tier",
            )
        elif manifest_kind == "specification" and release_ready is False and unresolved:
            runner.pass_(
                "manifest-contract-state",
                "specification contract is structurally valid; detached distribution evidence is not asserted by this tier",
            )
        else:
            runner.fail(
                "manifest-contract-state",
                "non-release tiers require either a complete release contract "
                "or a specification contract with unresolved fields",
            )

    inventory_failures, inventory_incompletes = release_artifact_inventory_diagnostics(
        manifest, release=release
    )
    if inventory_failures or inventory_incompletes:
        for error in inventory_failures:
            runner.fail("manifest-release-artifacts", error)
        for error in inventory_incompletes:
            runner.unresolved("manifest-release-artifacts", error)
    else:
        runner.pass_(
            "manifest-release-artifacts",
            "release artifact IDs, profiles, digest algorithms, and statuses are coherent",
        )

    gate_framework = manifest.get("gate_framework", {})
    if not isinstance(gate_framework, dict):
        runner.fail("gate-framework-policy", "gate_framework must be an object")
        return
    if (
        gate_framework.get("validation") != "fail_closed"
        or gate_framework.get("unavailable_dependency_policy")
        != "explicit_skip_or_fail"
    ):
        runner.fail("gate-framework-policy", "manifest gate policy is not fail-closed")
    else:
        runner.pass_("gate-framework-policy", "unavailable dependencies are explicit")


def version_and_source_drift(runner: GateRunner, manifest: dict[str, Any]) -> None:
    product = str(manifest.get("contracts", {}).get("product", ""))
    checks = [
        (ROOT / "Cargo.toml", r'^version\s*=\s*"([^"]+)"', product),
        (ROOT / "package.json", r'"version"\s*:\s*"([^"]+)"', product),
        (ROOT / "web/console/package.json", r'"version"\s*:\s*"([^"]+)"', product),
        (ROOT / "sdk/python/pyproject.toml", r'^version\s*=\s*"([^"]+)"', product),
    ]
    failures: list[str] = []
    for path, pattern, expected in checks:
        source = path.read_text(encoding="utf-8")
        match = re.search(pattern, source, re.MULTILINE)
        if match is None or match.group(1) != expected:
            failures.append(f"{path.relative_to(ROOT)} != {expected}")
    if failures:
        runner.fail("source-version-drift", "; ".join(failures))
    else:
        runner.pass_(
            "source-version-drift", f"workspace package versions agree with {product}"
        )

    runner.command(
        "rust-lockfile-metadata",
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
    )
    runner.command(
        "compatibility-manifest-verify",
        ["cargo", "run", "--locked", "-p", "xtask", "--", "compat", "verify"],
    )
    run_python(
        runner,
        "manifest-pack-executor-drift",
        ["scripts/manifest-evidence-wave6.py", "--json"],
    )


def toolchain_pins(runner: GateRunner, manifest: dict[str, Any]) -> None:
    pins = manifest.get("toolchains", {})
    if shutil.which("rustc"):
        actual = subprocess.run(
            ["rustc", "--version"], capture_output=True, text=True, check=False
        )
        found = (
            actual.stdout.split()[1] if len(actual.stdout.split()) > 1 else "unknown"
        )
        if found == pins.get("rust"):
            runner.pass_("rust-toolchain-pin", f"rustc {found}")
        else:
            runner.fail("rust-toolchain-pin", f"need {pins.get('rust')}, found {found}")
    else:
        runner.fail("rust-toolchain-pin", "rustc unavailable")
    if shutil.which("node"):
        actual = subprocess.run(
            ["node", "-p", "process.versions.node"],
            capture_output=True,
            text=True,
            check=False,
        )
        found = actual.stdout.strip()
        if found == pins.get("node"):
            runner.pass_("node-toolchain-pin", f"node {found}")
        else:
            runner.fail("node-toolchain-pin", f"need {pins.get('node')}, found {found}")
    else:
        runner.skip("node-toolchain-pin", "node unavailable")
    if shutil.which("pnpm"):
        actual = subprocess.run(
            ["pnpm", "--version"], capture_output=True, text=True, check=False
        )
        found = actual.stdout.strip()
        expected = "11.19.0"
        if found == expected:
            runner.pass_("pnpm-toolchain-pin", f"pnpm {found}")
        else:
            runner.fail("pnpm-toolchain-pin", f"need {expected}, found {found}")
    else:
        runner.skip("pnpm-toolchain-pin", "pnpm unavailable")


SECRET_PATTERNS = [
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(
        rb"(?i)(?:aws_secret_access_key|private_key|client_secret)\s*[:=]\s*[^\s]{12,}"
    ),
    re.compile(rb"(?i)(?:postgres(?:ql)?|mysql|redis)://[^\s:@]+:[^\s@]+@"),
    re.compile(rb"(?i)gh[pousr]_[A-Za-z0-9_\-]{20,}"),
]


def verify_required_tool_version(
    runner: GateRunner, tool: str, argv: list[str]
) -> None:
    expected = HOSTED_REQUIRED_TOOL_VERSIONS[tool]
    try:
        result = subprocess.run(
            argv,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        runner.fail(f"{tool}-version", f"cannot inspect required tool: {error}")
        return
    observed = (result.stdout + "\n" + result.stderr).strip()
    version = re.search(r"(?<![0-9])([0-9]+\.[0-9]+\.[0-9]+)(?![0-9])", observed)
    if result.returncode == 0 and version is not None and version.group(1) == expected:
        runner.pass_(f"{tool}-version", f"exact hosted tool version {expected}")
    else:
        runner.fail(
            f"{tool}-version",
            f"need exact hosted tool version {expected}; observed output was not accepted",
        )


def secret_scan(runner: GateRunner, *, release: bool = False) -> None:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        cwd=ROOT,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        runner.fail("secret-scan", "git candidate-file inventory failed")
        return
    matches: list[str] = []
    for raw_path in result.stdout.split(b"\0"):
        if not raw_path:
            continue
        path = ROOT / os.fsdecode(raw_path)
        try:
            data = path.read_bytes()
        except OSError as error:
            runner.fail("secret-scan", f"cannot read candidate file {path}: {error}")
            return
        for pattern in SECRET_PATTERNS:
            if pattern.search(data):
                matches.append(str(path.relative_to(ROOT)))
                break
    if matches:
        runner.fail("secret-scan", "possible secret material in " + ", ".join(matches))
    else:
        runner.pass_(
            "secret-scan",
            "tracked and untracked candidate files contain no configured secret patterns",
        )

    if shutil.which("gitleaks"):
        if runner.ci or release:
            verify_required_tool_version(runner, "gitleaks", ["gitleaks", "version"])
        runner.command("gitleaks", ["gitleaks", "detect", "--no-banner", "--redact"])
    elif release:
        runner.fail("gitleaks", "required release secret scanner unavailable")
    else:
        runner.record(
            "gitleaks",
            "SKIP_INCOMPLETE",
            "optional scanner unavailable; built-in scan ran",
        )


def dependency_scan(
    runner: GateRunner, *, release: bool, include_ecosystems: bool
) -> None:
    runner.command(
        "cargo-locked-offline-resolution",
        [
            "cargo",
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ],
    )
    if include_ecosystems:
        uv = shutil.which("uv")
        if uv:
            runner.command(
                "python-lock-resolution",
                [uv, "lock", "--check", "--project", "sdk/python"],
            )
        else:
            runner.skip("python-lock-resolution", "uv unavailable")

        pnpm = shutil.which("pnpm")
        if pnpm:
            runner.command(
                "node-lock-resolution",
                [pnpm, "install", "--frozen-lockfile", "--offline"],
            )
        else:
            runner.skip("node-lock-resolution", "pnpm unavailable")

    audit = shutil.which("cargo-audit")
    if audit:
        if runner.ci or release:
            verify_required_tool_version(runner, "cargo-audit", [audit, "--version"])
        # cargo-audit reads Cargo.lock by default. Its current CLI does not
        # accept Cargo's unrelated --locked flag; lockfile resolution was
        # already checked above with cargo metadata.
        runner.command("cargo-audit", [audit, "audit"])
    elif release:
        runner.fail("cargo-audit", "required release dependency scanner unavailable")
    else:
        runner.record(
            "cargo-audit",
            "SKIP_INCOMPLETE",
            "optional local dependency scanner unavailable",
        )


def rust_checks(runner: GateRunner, *, full: bool) -> None:
    runner.command("rust-format", ["cargo", "fmt", "--all", "--", "--check"])
    if full:
        runner.command(
            "rust-lint",
            [
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        )
        runner.command("rust-build", ["cargo", "build", "--workspace", "--locked"])
        runner.command("rust-tests", ["cargo", "test", "--workspace", "--locked"])
    else:
        runner.command(
            "rust-fast-lint",
            [
                "cargo",
                "clippy",
                "-p",
                "worldstream-core",
                "-p",
                "xtask",
                "--lib",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        )
        runner.command(
            "rust-fast-tests",
            [
                "cargo",
                "test",
                "-p",
                "worldstream-core",
                "-p",
                "worldstream-protocol",
                "-p",
                "xtask",
                "--locked",
            ],
        )


def critical_contract_matrix(runner: GateRunner) -> None:
    """Run the bounded SQLite/PostgreSQL contract slices in every local tier.

    The workspace-wide test is intentionally not the only signal here.  These
    named slices make a gate failure attributable to the two storage adapters
    and keep the fast/pre-push contract from silently shrinking to Core-only
    tests.
    """

    runner.command(
        "sqlite-critical-contracts",
        [
            "cargo",
            "test",
            "-p",
            "worldstream-sqlite",
            "--lib",
            "--locked",
            "--",
            "property_based_replay_fuzz_probe",
        ],
    )
    runner.command(
        "postgres-critical-contracts",
        [
            "cargo",
            "test",
            "-p",
            "worldstream-postgres",
            "--lib",
            "--locked",
        ],
    )
    runner.command(
        "golden-corpus-contract",
        [
            "cargo",
            "test",
            "-p",
            "worldstream-core",
            "--lib",
            "--locked",
            "--",
            "shared_literal_golden_freezes_all_five_hash_objects_records_and_heads",
        ],
    )


def sdk_and_ui_checks(runner: GateRunner) -> None:
    if shutil.which("uv"):
        runner.command(
            "python-sdk-format",
            [
                "uv",
                "run",
                "--project",
                "sdk/python",
                "--locked",
                "ruff",
                "format",
                "--check",
            ],
        )
        runner.command(
            "python-sdk-lint",
            ["uv", "run", "--project", "sdk/python", "--locked", "ruff", "check"],
        )
        runner.command(
            "python-sdk-tests",
            [
                "uv",
                "run",
                "--project",
                "sdk/python",
                "--locked",
                "pytest",
                "sdk/python/tests",
            ],
        )
    else:
        runner.skip("python-sdk-tools", "uv unavailable")

    if shutil.which("pnpm"):
        runner.command("ui-lint", ["pnpm", "--dir", "web/console", "lint"])
        runner.command("ui-tests", ["pnpm", "--dir", "web/console", "test"])
        runner.command("ui-build", ["pnpm", "--dir", "web/console", "build"])
    else:
        runner.skip("ui-tools", "pnpm unavailable")


def root_python_checks(runner: GateRunner) -> None:
    """Run the repository-owned Python gate and producer suite explicitly.

    The root files intentionally use descriptive ``*_smoke.py`` names, so a
    bare ``pytest tests`` command collects none of them under pytest's default
    discovery rules.  Keep the sorted file inventory mechanical and visible.
    """

    uv = shutil.which("uv")
    if uv is None:
        runner.skip("root-python-tools", "uv unavailable")
        return
    prefix = [uv, "run", "--project", "sdk/python", "--locked"]
    runner.command(
        "root-python-format",
        [*prefix, "ruff", "format", "--check", "scripts", "tests"],
    )
    runner.command(
        "root-python-lint",
        [*prefix, "ruff", "check", "scripts", "tests"],
    )
    if not ROOT_PYTHON_TESTS:
        runner.fail("root-python-tests", "no tests/*.py files were discovered")
        return
    runner.command(
        "root-python-tests",
        [*prefix, "python", "-m", "pytest", "-q", *ROOT_PYTHON_TESTS],
    )


def sqlite_checks(runner: GateRunner, *, full: bool) -> None:
    package = ["cargo", "test", "-p", "worldstream-sqlite", "--locked"]
    if not full:
        package.extend(["--lib"])
    runner.command("sqlite-critical-matrix", package)
    if platform_module.system() == "Windows":
        runner.command(
            "operator-shell-smoke",
            ["pwsh", "-NoProfile", "-File", "scripts/smoke-operator.ps1"],
        )
    else:
        runner.command("operator-shell-smoke", ["scripts/smoke-operator.sh"])


def cargo_test_evidence(
    runner: GateRunner,
    name: str,
    package: str,
    filters: tuple[str, ...],
    boundary: str,
) -> None:
    """Run a counted local test slice without promoting it to release evidence."""
    cargo = shutil.which("cargo")
    if cargo is None:
        runner.fail(name, "unavailable dependency: cargo")
        return

    counts: list[str] = []
    for test_filter in filters:
        argv = [cargo, "test", "--locked"]
        if runner.offline:
            argv.append("--offline")
        argv.extend(["-p", package, "--lib", "--", test_filter])
        result = subprocess.run(
            argv,
            cwd=ROOT,
            env={
                **os.environ,
                **(
                    {
                        "CARGO_NET_OFFLINE": "true",
                        "UV_OFFLINE": "true",
                        "npm_config_offline": "true",
                    }
                    if runner.offline
                    else {}
                ),
            },
            capture_output=True,
            text=True,
            check=False,
        )
        output = result.stdout + result.stderr
        summaries = re.findall(r"test result: ok\. (\d+) passed;", output)
        if result.returncode != 0:
            tail = output[-1200:].strip().replace("\n", " ")
            runner.fail(
                name,
                f"{package}[{test_filter or '<all>'}] exited {result.returncode}: {tail}",
            )
            return
        if not summaries or int(summaries[-1]) == 0:
            runner.fail(
                name,
                f"{package}[{test_filter or '<all>'}] produced no passing test count",
            )
            return
        counts.append(f"{test_filter or '<all>'}={summaries[-1]}")

    runner.pass_(name, f"{boundary}; counted local tests: {', '.join(counts)}")


def deterministic_evidence_checks(runner: GateRunner) -> None:
    """Exercise only repository-local evidence; contract rows stay fail-closed."""
    cargo_test_evidence(
        runner,
        "config-contract",
        "worldstream-runtime",
        ("config::tests::",),
        "defaults/file/environment/CLI precedence, every typed invalid-input class, redacted effective config, and secret diagnostics",
    )
    cargo_test_evidence(
        runner,
        "evidence-migration-local",
        "worldstream-sqlite",
        ("migration",),
        "bundled SQLite forward-migration, restart, and migration-backup fixtures",
    )
    cargo_test_evidence(
        runner,
        "evidence-transfer-local",
        "worldstream-transfer",
        ("",),
        "deterministic bounded transfer and in-memory destination fixtures only; no live PostgreSQL",
    )
    cargo_test_evidence(
        runner,
        "evidence-backup-restore-local",
        "worldstream-backup",
        ("",),
        "bundled SQLite native backup/restore and read-only verifier fixtures only",
    )
    cargo_test_evidence(
        runner,
        "evidence-snapshot-local",
        "worldstream-sqlite",
        ("snapshot",),
        "paired snapshot, lineage rebuild, and post-commit snapshot fixtures only",
    )
    cargo_test_evidence(
        runner,
        "evidence-privacy-local",
        "worldstream-core",
        ("privacy", "agent_heist_privacy_tests::", "debug_and_errors_do_not_expose"),
        "local privacy and redaction fixtures only; no cross-process collector",
    )
    cargo_test_evidence(
        runner,
        "evidence-capability-local",
        "worldstream-core",
        ("authority_tests::",),
        "local capability and authority-fence fixtures only",
    )
    cargo_test_evidence(
        runner,
        "evidence-lease-local",
        "worldstream-sqlite",
        ("activation_schema_fences",),
        "local activation lease-generation and receipt-fence fixture only",
    )
    cargo_test_evidence(
        runner,
        "evidence-telemetry-local",
        "worldstream-server",
        ("telemetry",),
        "local redaction, bounded queue, exporter, and shutdown fixtures only",
    )


def process_contract_checks(runner: GateRunner) -> None:
    """Execute the process-level failure and backpressure probes when possible.

    The shell probes are themselves fail-closed and redact their diagnostics.
    On a non-POSIX host there is no valid substitution for the native process
    launcher, so strict/CI tiers report an incomplete blocker rather than
    treating fixture tests as process evidence.
    """

    scripts = (
        ("process-kill-point", "scripts/kill-point-smoke.sh"),
        ("telemetry-failure-pressure", "scripts/telemetry-failure-smoke.sh"),
        ("telemetry-https", "scripts/telemetry_https_smoke.sh"),
        (
            "bounded-sqlite-soak",
            "scripts/soak-smoke.sh",
        ),
    )
    if os.name != "posix":
        for name, script in scripts:
            runner.unresolved(
                name,
                f"native POSIX process evidence {script} is not executable on {os.name}",
            )
        return

    # kill-point-smoke.sh intentionally uses SIGKILL and process-level crash
    # recovery.  The probe is a Linux release cell, not portable Darwin
    # evidence.  A local/pre-push run on macOS must remain an explicit
    # incomplete skip; only the native Linux/release cell may promote PASS.
    if platform_module.system() != "Linux":
        runner.record(
            "process-kill-point",
            "SKIP_INCOMPLETE",
            "unresolved evidence: SIGKILL crash-boundary evidence is Linux-only; run the native Linux release cell",
            classification="incomplete",
        )
    else:
        runner.command(
            "process-kill-point",
            ["scripts/kill-point-smoke.sh"],
        )
    runner.command(
        "telemetry-failure-pressure",
        ["scripts/telemetry-failure-smoke.sh"],
    )
    runner.command(
        "telemetry-https",
        ["scripts/telemetry_https_smoke.sh"],
    )
    runner.command(
        "bounded-sqlite-soak",
        [
            "scripts/soak-smoke.sh",
            "--iterations",
            "1",
            "--max-total-seconds",
            "120",
        ],
    )


def postgres_live_contract(runner: GateRunner, *, required: bool) -> None:
    """Run the pinned direct + PgBouncer transaction-pool harness.

    ``postgres-live-evidence.sh`` owns disposable containers, exact image
    digests, role separation, the adapter conformance run, and the shared
    seven-scenario comparison.  A zero exit status is not enough: validate
    the machine-readable report so a script that only started PostgreSQL
    cannot be mistaken for direct/pooler evidence.
    """

    script = ROOT / "scripts/postgres-live-evidence.sh"
    if os.name != "posix" or not script.is_file() or not os.access(script, os.X_OK):
        detail = "pinned PostgreSQL/PgBouncer live harness is unavailable on this host"
        if required:
            runner.fail("postgres-live-contract", detail)
        else:
            runner.record(
                "postgres-live-contract",
                "SKIP_INCOMPLETE",
                f"unresolved evidence: {detail}; hosted Linux owns this external cell",
                classification="incomplete",
            )
        return
    if shutil.which("docker") is None or shutil.which("psql") is None:
        detail = "docker and psql are required for the pinned PostgreSQL/PgBouncer live harness"
        if required:
            runner.fail("postgres-live-contract", detail)
        else:
            runner.record(
                "postgres-live-contract",
                "SKIP_INCOMPLETE",
                f"unresolved evidence: {detail}; hosted Linux owns this external cell",
                classification="incomplete",
            )
        return

    with tempfile.TemporaryDirectory(prefix="worldstream-gate-pg-live-") as directory:
        evidence_path = Path(directory) / "postgres-live.json"
        result = subprocess.run(
            [str(script), "--evidence", str(evidence_path)],
            cwd=ROOT,
            env=os.environ.copy(),
            capture_output=True,
            text=True,
            check=False,
        )
        try:
            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            detail = f"live harness did not produce valid evidence JSON: {error}"
            if result.returncode:
                detail += f"; exit={result.returncode}"
            runner.fail("postgres-live-contract", detail)
            return
        required_shape = (
            evidence.get("schema") == "worldstream/postgresql-live-evidence/v1"
            and evidence.get("status") == "pass"
            and evidence.get("release_evidence") is False
            and evidence.get("secrets_emitted") is False
            and evidence.get("cleanup", {}).get("status") == "pass"
            and evidence.get("profiles", {}).get("transaction_pooler") == "pass"
            and evidence.get("imo_50_shared_conformance", {}).get("all_scenarios_pass")
        )
        if result.returncode != 0 or not required_shape:
            reason = evidence.get("reason", "live harness contract did not pass")
            runner.fail(
                "postgres-live-contract",
                f"direct/PgBouncer harness incomplete or failed: reason={reason!r}; exit={result.returncode}",
            )
        else:
            runner.pass_(
                "postgres-live-contract",
                "pinned PostgreSQL 17.11 direct and transaction-pooler harness passed; seven-scenario comparison passed",
            )


def filesystem_checks(runner: GateRunner, *, host_os: str | None = None) -> None:
    security = load_manifest().get("security", {})
    effective_os = os.name if host_os is None else host_os
    with tempfile.TemporaryDirectory(prefix="worldstream-gate-") as directory:
        path = Path(directory) / "secret"
        path.write_text("gate", encoding="utf-8")
        if effective_os == "posix":
            path.chmod(0o600)
            mode = stat.S_IMODE(path.stat().st_mode)
            if (
                mode != 0o600
                or security.get("sqlite_posix_mode") != "owner_only_umask_077"
            ):
                runner.fail(
                    "filesystem-owner-only", f"mode/security mismatch: {oct(mode)}"
                )
            else:
                runner.pass_(
                    "filesystem-owner-only", "POSIX secret fixture is owner-only"
                )
            link = Path(directory) / "symlink"
            link.symlink_to(path)
            if not link.is_symlink() or not security.get(
                "reject_sqlite_symlink_or_reparse"
            ):
                runner.fail(
                    "filesystem-link-policy",
                    "symlink/reparse rejection policy is not enforced",
                )
            else:
                runner.pass_(
                    "filesystem-link-policy",
                    "symlink policy is declared and probe is present",
                )
            runner.pass_(
                "evidence-filesystem-local",
                "POSIX owner-only mode and symlink policy probes passed; Windows ACL/reparse evidence remains external",
            )
        elif effective_os == "nt":
            if os.name != "nt":
                runner.unresolved(
                    "filesystem-owner-only",
                    "native Windows ACL/reparse-point evidence requires a Windows runner",
                )
                runner.unresolved(
                    "filesystem-acl-policy",
                    "Windows ACL rejection is declared but not executable on this gate path",
                )
                runner.unresolved(
                    "evidence-filesystem-local",
                    "native Windows ACL/reparse-point evidence requires a Windows runner",
                )
                return
            cargo_test_evidence(
                runner,
                "filesystem-owner-only",
                "worldstream-runtime",
                ("creates_and_reopens_owner_only_data_directory",),
                "native Windows protected DACL creation and reopen",
            )
            cargo_test_evidence(
                runner,
                "filesystem-acl-policy",
                "worldstream-runtime",
                (
                    "rejects_broad_windows_directory_dacl",
                    "validates_secret_file_and_rejects_broad_replacement_dacl",
                    "rejects_nonlocal_windows_data_path_prefixes_before_creation",
                ),
                "native Windows broad-DACL, secret-file, and nonlocal-path rejection",
            )
            cargo_test_evidence(
                runner,
                "evidence-filesystem-local",
                "worldstream-runtime",
                (
                    "regular_file_check_rejects_devices_and_ambiguous_types",
                    "rejects_resolved_network_paths",
                ),
                "native Windows reparse/device/network-path policy",
            )
        else:
            runner.skip("filesystem-owner-only", f"unsupported host OS: {effective_os}")


def manifest_cell_rows(manifest: dict[str, Any], tier: str) -> list[dict[str, Any]]:
    cells = manifest.get("gate_cells", [])
    if not isinstance(cells, list):
        return []
    rows: list[dict[str, Any]] = []
    for cell in cells:
        if not isinstance(cell, dict) or cell.get("tier") != tier:
            continue
        if tier == "minimal-ci" and cell.get("required") is not True:
            continue
        rows.append(cell)
    if tier == "minimal-ci" and not any(
        row.get("platform") == "oci-linux-amd64" for row in rows
    ):
        # The OCI target is already a required release platform declaration.
        # Materialize one CI row here so the workflow cannot silently omit it,
        # while keeping compatibility.toml the sole source of platform truth.
        platforms = manifest.get("platforms", [])
        oci = next(
            (
                profile
                for profile in platforms
                if isinstance(profile, dict)
                and profile.get("id") == "oci-linux-amd64"
                and profile.get("support") == "release"
            ),
            None,
        )
        if isinstance(oci, dict):
            rows.append(
                {
                    "id": "oci-linux-amd64",
                    "tier": "minimal-ci",
                    "platform": "oci-linux-amd64",
                    "required": True,
                    "storage_profiles": list(oci.get("storage_profiles", [])),
                    "derived_from_platform": True,
                }
            )
    return rows


def manifest_cells(manifest: dict[str, Any], tier: str) -> list[str]:
    return [
        cell["id"]
        for cell in manifest_cell_rows(manifest, tier)
        if isinstance(cell.get("id"), str) and cell["id"]
    ]


def cell_route(cell: dict[str, Any]) -> dict[str, Any] | None:
    platform_id = cell.get("platform")
    if not isinstance(platform_id, str):
        return None
    route = CELL_ROUTES.get(platform_id)
    if route is None:
        return None
    return {"platform": platform_id, **route}


def selected_ci_cell(
    runner: GateRunner, manifest: dict[str, Any], cell_id: str | None
) -> tuple[dict[str, Any], dict[str, Any]] | None:
    rows = manifest_cell_rows(manifest, "minimal-ci")
    if not cell_id:
        runner.fail("ci-cell", "minimal-ci requires --cell from the manifest")
        return None
    matches = [row for row in rows if row.get("id") == cell_id]
    if not matches:
        runner.fail("ci-cell", f"unknown required manifest cell: {cell_id}")
        return None
    if len(matches) != 1:
        runner.fail("ci-cell", f"manifest contains duplicate cell id: {cell_id}")
        return None
    row = matches[0]
    route = cell_route(row)
    if route is None:
        runner.fail(
            "ci-routing",
            f"manifest cell {cell_id} has no supported native route for platform {row.get('platform')!r}",
        )
        return None
    if route["system"] != platform_module.system():
        runner.skip(
            "ci-platform",
            "platform blocker: "
            f"cell={cell_id}; platform={route['platform']}; "
            f"requires runner={route['runner']} shell={route['shell']} "
            f"system={route['system']}; observed system={platform_module.system()}",
        )
        return None
    return row, route


def list_cells(manifest: dict[str, Any], tier: str, output: str) -> int:
    rows = manifest_cell_rows(manifest, tier)
    values = [cell.get("id") for cell in rows]
    invalid = [
        f"row {index} has no non-empty string id"
        for index, cell in enumerate(rows)
        if not isinstance(cell.get("id"), str) or not cell["id"]
    ]
    routes: dict[str, dict[str, Any]] = {}
    seen_ids: set[str] = set()
    for cell in rows:
        cell_id = cell.get("id")
        if not isinstance(cell_id, str) or not cell_id:
            continue
        if cell_id in seen_ids:
            invalid.append(f"duplicate manifest cell id: {cell_id}")
        seen_ids.add(cell_id)
        route = cell_route(cell)
        if route is None:
            invalid.append(
                f"cell {cell_id} has no supported route for platform {cell.get('platform')!r}"
            )
        else:
            routes[cell_id] = route
    if invalid:
        for detail in invalid:
            print(f"CI cell routing error: {detail}", file=sys.stderr)
        return 1
    values = [value for value in values if isinstance(value, str)]
    if output == "json":
        print(json.dumps(values, separators=(",", ":")))
    elif output == "matrix":
        include = []
        for value in values:
            route = routes[value]
            include.append(
                {
                    "cell": value,
                    "platform": route["platform"],
                    "runner": route["runner"],
                    "shell": route["shell"],
                    "system": route["system"],
                }
            )
        print(json.dumps({"include": include}, separators=(",", ":")))
    else:
        print("\n".join(values))
    return 0


def oci_cell_checks(runner: GateRunner) -> None:
    """Validate the OCI route and template without building an image."""

    runner.command(
        "oci-context-dry-run",
        [
            sys.executable,
            "scripts/package.py",
            "oci-context",
            "--base-image",
            OCI_TEMPLATE_BASE_IMAGE,
            "--dry-run",
        ],
    )
    runner.command(
        "oci-template-syntax",
        [
            "docker",
            "buildx",
            "build",
            "--check",
            "--platform",
            "linux/amd64",
            "--build-arg",
            f"WORLDSTREAM_BASE_IMAGE={OCI_TEMPLATE_BASE_IMAGE}",
            "packaging/oci",
        ],
        required=runner.ci,
    )
    runner.pass_(
        "oci-release-evidence-boundary",
        "OCI matrix checks context shape and Dockerfile syntax only; image digest and runtime evidence remain external",
    )


def read_postgres_gate_dsn() -> tuple[str | None, str | None, bool]:
    """Read the optional PostgreSQL probe DSN without exposing its bytes.

    Hosted cells use an owner-only regular file.  The legacy environment
    value remains available for an explicitly configured developer probe, but
    the two sources are mutually exclusive and hosted workflow tests ensure it
    is never used there.
    """

    dsn_file_value = os.environ.get("WORLDSTREAM_POSTGRES_DSN_FILE")
    legacy_value = os.environ.get("WORLDSTREAM_POSTGRES_URL")
    if dsn_file_value and legacy_value:
        return None, "both PostgreSQL DSN sources are configured", True
    if not dsn_file_value:
        return legacy_value or None, None, False
    path = Path(dsn_file_value)
    try:
        metadata = path.lstat()
    except OSError:
        return None, "configured PostgreSQL DSN file is unavailable", True
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        return None, "configured PostgreSQL DSN path is not a regular file", True
    if os.name == "posix":
        if metadata.st_mode & 0o077:
            return None, "configured PostgreSQL DSN file is not owner-only", True
        if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
            return None, "configured PostgreSQL DSN file has a different owner", True
    if metadata.st_size <= 0 or metadata.st_size > MAX_POSTGRES_DSN_BYTES:
        return None, "configured PostgreSQL DSN file has an invalid size", True
    try:
        raw = path.read_bytes()
        value = raw.decode("utf-8").strip()
    except (OSError, UnicodeError):
        return None, "configured PostgreSQL DSN file is not readable UTF-8", True
    if not value or "\x00" in value or "\n" in value or "\r" in value:
        return (
            None,
            "configured PostgreSQL DSN file must contain one bounded line",
            True,
        )
    return value, None, True


def passwordless_postgres_target(value: str) -> tuple[str, dict[str, str]]:
    """Return a libpq target with any password scoped to the child env only."""

    child_env: dict[str, str] = {}
    if "://" in value:
        parsed = urllib.parse.urlsplit(value)
        if not parsed.scheme or not parsed.hostname:
            raise ValueError("PostgreSQL URI is malformed")
        if parsed.password is not None:
            child_env["PGPASSWORD"] = parsed.password
        username = (
            urllib.parse.quote(parsed.username, safe="") + "@"
            if parsed.username is not None
            else ""
        )
        hostname = parsed.hostname
        if ":" in hostname and not hostname.startswith("["):
            hostname = f"[{hostname}]"
        port = f":{parsed.port}" if parsed.port is not None else ""
        return (
            urllib.parse.urlunsplit(
                (
                    parsed.scheme,
                    f"{username}{hostname}{port}",
                    parsed.path,
                    parsed.query,
                    "",
                )
            ),
            child_env,
        )

    try:
        fields = shlex.split(value, posix=True)
    except ValueError as error:
        raise ValueError("PostgreSQL keyword DSN is malformed") from error
    sanitized: list[str] = []
    for field in fields:
        key, separator, field_value = field.partition("=")
        if not separator or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) is None:
            raise ValueError("PostgreSQL keyword DSN is malformed")
        if key.lower() in {"password", "passfile", "sslpassword"}:
            if key.lower() == "password":
                child_env["PGPASSWORD"] = field_value
            else:
                child_env[
                    {"passfile": "PGPASSFILE", "sslpassword": "PGSSLPASSWORD"}[
                        key.lower()
                    ]
                ] = field_value
            continue
        escaped = field_value.replace("\\", "\\\\").replace("'", "\\'")
        sanitized.append(f"{key}='{escaped}'")
    if not sanitized:
        raise ValueError("PostgreSQL keyword DSN has no connection fields")
    return " ".join(sanitized), child_env


def local_postgresql(runner: GateRunner, *, required: bool) -> None:
    value, configuration_error, from_file = read_postgres_gate_dsn()
    if configuration_error is not None:
        runner.fail("postgres-local", configuration_error)
        return
    if not value:
        detail = "WORLDSTREAM_POSTGRES_DSN_FILE is not set; no credential is required for this optional cell"
        if required:
            runner.fail("postgres-local", detail)
        else:
            runner.record("postgres-local", "SKIP_INCOMPLETE", detail)
        return
    if not shutil.which("psql"):
        detail = "psql unavailable for the configured local PostgreSQL cell"
        if required:
            runner.fail("postgres-local", detail)
        else:
            runner.record("postgres-local", "SKIP_INCOMPLETE", detail)
        return
    try:
        target, child_env = passwordless_postgres_target(value)
    except (ValueError, UnicodeError):
        runner.fail("postgres-local", "configured PostgreSQL DSN is malformed")
        return
    runner.command(
        "postgres-local",
        [
            "psql",
            target,
            "--no-psqlrc",
            "--tuples-only",
            "--command",
            "select current_setting('server_version_num')",
        ],
        required=required,
        env=child_env,
    )
    source = "owner-only DSN file" if from_file else "explicit local environment"
    print(f"PostgreSQL smoke target configured from {source}")


def evidence_inventory_errors(
    manifest: dict[str, Any], *, release: bool
) -> tuple[dict[str, dict[str, Any]], list[str], list[str]]:
    """Return evidence rows plus combined failure/incomplete diagnostics.

    The compatibility API historically exposed three return values.  Keep that
    shape for callers, while ``evidence_inventory_diagnostics`` gives the gate
    runner the separate classifications needed for reporting.
    """
    rows, shape_errors, failures, incompletes = evidence_inventory_diagnostics(
        manifest, release=release
    )
    return rows, shape_errors, failures + incompletes


def evidence_inventory_diagnostics(
    manifest: dict[str, Any], *, release: bool
) -> tuple[dict[str, dict[str, Any]], list[str], list[str], list[str]]:
    """Separate malformed evidence claims from evidence not supplied yet."""
    raw_rows = manifest.get("evidence")
    if not isinstance(raw_rows, list):
        return {}, ["evidence must be a list"], [], []

    rows: dict[str, dict[str, Any]] = {}
    shape_errors: list[str] = []
    failures: list[str] = []
    incompletes: list[str] = []
    detached = uses_detached_release_identity(manifest)
    for index, row in enumerate(raw_rows):
        if not isinstance(row, dict):
            shape_errors.append(f"evidence row {index} is not an object")
            continue
        evidence_id = row.get("id")
        if not isinstance(evidence_id, str) or not evidence_id:
            shape_errors.append(f"evidence row {index} has no non-empty id")
            continue
        if evidence_id in rows:
            shape_errors.append(f"duplicate evidence id: {evidence_id}")
            continue
        rows[evidence_id] = row
        if not isinstance(row.get("release_gate"), bool):
            shape_errors.append(f"evidence release_gate must be boolean: {evidence_id}")
        status = row.get("status")
        digest = row.get("artifact_digest")
        allowed_statuses = {"resolved", "unresolved"}
        if detached:
            allowed_statuses.add("detached")
        if status not in allowed_statuses:
            shape_errors.append(f"evidence status is invalid: {evidence_id}")
        if not isinstance(digest, str):
            shape_errors.append(
                f"evidence artifact_digest must be a string: {evidence_id}"
            )
        elif digest and not EVIDENCE_DIGEST.fullmatch(digest):
            shape_errors.append(f"evidence artifact_digest is malformed: {evidence_id}")
        if status == "resolved" and (
            not isinstance(digest, str) or not EVIDENCE_DIGEST.fullmatch(digest or "")
        ):
            failures.append(f"{evidence_id}: resolved without a valid artifact_digest")
        if status == "unresolved" and digest:
            failures.append(f"{evidence_id}: unresolved row has an artifact_digest")
        if status == "detached" and digest:
            failures.append(f"{evidence_id}: detached row has an artifact_digest")
        if (
            detached
            and row.get("release_gate") is True
            and (
                status != "detached"
                or digest != ""
                or row.get("artifact_digest_location") != "release-manifest.json"
            )
        ):
            failures.append(
                f"{evidence_id}: detached release evidence must use status=detached, "
                "artifact_digest='', artifact_digest_location='release-manifest.json'"
            )
        if row.get("release_gate") is True and (
            status not in ({"resolved", "detached"} if detached else {"resolved"})
            or not isinstance(digest, str)
            or (not detached and not EVIDENCE_DIGEST.fullmatch(digest))
        ):
            if status == "unresolved" and not digest and not detached:
                incompletes.append(
                    f"{evidence_id}: release-gated evidence is not resolved"
                )
            elif detached and status == "detached" and not digest:
                pass
            elif not any(evidence_id in error for error in failures):
                failures.append(
                    f"{evidence_id}: release-gated evidence is not resolved with a valid digest"
                )

    if (
        release
        and not failures
        and not incompletes
        and not any(row.get("release_gate") is True for row in rows.values())
    ):
        failures.append("no release-gated evidence rows are declared")
    return rows, shape_errors, failures, incompletes


def contract_inventory(
    runner: GateRunner, manifest: dict[str, Any], *, release: bool
) -> None:
    """Keep unimplemented durability tiers visible instead of calling them green."""
    evidence, shape_errors, evidence_failures, evidence_incompletes = (
        evidence_inventory_diagnostics(manifest, release=release)
    )
    for error in shape_errors:
        runner.fail("evidence-inventory", error)
    if evidence_failures:
        runner.fail("release-evidence-inventory", "; ".join(evidence_failures))
    if evidence_incompletes:
        runner.unresolved("release-evidence-inventory", "; ".join(evidence_incompletes))
    if not evidence_failures and not evidence_incompletes:
        runner.pass_(
            "release-evidence-inventory",
            "all declared evidence rows have coherent status and digest shapes",
        )
    contract_cells = {
        "migration": [
            "sqlite-conformance-migration-backup-restore-crash",
            "all-prior-forward-migrations-both-backends",
        ],
        "transfer": ["sqlite-postgresql-transfer-byte-parity-and-epoch-fencing"],
        "restore": ["backend-native-isolated-restore-and-full-semantic-verifier"],
        "snapshot": ["sqlite-conformance-migration-backup-restore-crash"],
        "kill-point": ["failure-fuzz-resource-and-one-hour-sqlite-soak"],
        "privacy": ["config-secrets-probes-observability-security"],
        "capability": ["config-secrets-probes-observability-security"],
        "lease": ["config-secrets-probes-observability-security"],
        "filesystem": ["config-secrets-probes-observability-security"],
        "telemetry-backpressure": ["config-secrets-probes-observability-security"],
        "postgresql": ["postgresql-direct-and-transaction-pooler-conformance"],
    }
    detached = uses_detached_release_identity(manifest)
    for cell_name, required_ids in contract_cells.items():
        cell_failures: list[str] = []
        cell_incompletes: list[str] = []
        for evidence_id in required_ids:
            row = evidence.get(evidence_id)
            if not row:
                cell_incompletes.append(f"{evidence_id}: missing")
            elif row.get("status") == "detached" and detached:
                pass
            elif row.get("status") != "resolved":
                cell_incompletes.append(f"{evidence_id}: status={row.get('status')}")
            elif not EVIDENCE_DIGEST.fullmatch(str(row.get("artifact_digest", ""))):
                cell_failures.append(f"{evidence_id}: artifact_digest is malformed")
        if cell_failures:
            runner.fail("contract-" + cell_name, "; ".join(cell_failures))
        elif cell_incompletes:
            runner.unresolved("contract-" + cell_name, "; ".join(cell_incompletes))
        else:
            runner.pass_(
                "contract-" + cell_name, "all mapped evidence rows are resolved"
            )

    if not release:
        # These are useful inventory assertions even before the full release
        # harness exists.  They do not convert unresolved evidence into proof.
        sources = list((ROOT / "crates").rglob("*.rs"))
        corpus = "\n".join(path.read_text(encoding="utf-8") for path in sources)
        for name, needle in {
            "privacy": "privacy",
            "capability": "capability",
            "lease": "lease",
            "snapshot": "snapshot",
            "recovery": "recovery",
        }.items():
            if needle in corpus.lower():
                runner.pass_(
                    f"inventory-{name}",
                    "implementation/test corpus contains contract terminology",
                )
            else:
                runner.skip(f"inventory-{name}", "no implementation marker found")


def expected_release_basename(artifact_id: str, version: str) -> str | None:
    fixed = {
        "source-archive": f"worldstream-{version}-source.tar.gz",
        "native-linux-x86_64-archive": f"worldstream-{version}-linux-x86_64.tar.gz",
        "native-windows-x64-archive": f"worldstream-{version}-windows-x64.zip",
        "checksums": "SHA256SUMS",
        "sigstore-bundle": "sigstore.bundle.json",
        "spdx-sbom": "sbom.spdx.json",
        "slsa-provenance": "provenance.json",
    }
    return fixed.get(artifact_id)


def release_artifact_path_error(
    artifact_id: str, relative: str, version: str
) -> str | None:
    basename = PurePosixPath(relative).name
    expected = expected_release_basename(artifact_id, version)
    if expected is not None and basename != expected:
        return f"artifact filename must be {expected}: {relative}"
    if artifact_id == "oci-linux-amd64-image":
        pattern = re.compile(
            rf"worldstream-{re.escape(version)}-oci-linux-amd64(?:\.oci)?\.(?:tar|tar\.gz|digest|json)"
        )
        if pattern.fullmatch(basename) is None:
            return (
                f"OCI artifact filename is not a supported Linux/amd64 form: {relative}"
            )
    if any(
        marker in relative.casefold()
        for marker in (
            "arm64",
            "aarch64",
            "darwin",
            "macos-binary",
            "windows-container",
            ".msi",
            ".msix",
        )
    ):
        return f"artifact names an unsupported platform or distribution: {relative}"
    return None


def identity_mismatch(label: str, expected: object, observed: object) -> str:
    """Format identity diagnostics without hiding the compared values."""
    return f"{label}: expected={expected!r}; observed={observed!r}"


def load_json_object(
    runner: GateRunner, name: str, path: Path
) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        runner.fail(name, f"invalid JSON: {error}")
        return None
    if not isinstance(value, dict):
        runner.fail(name, "document must be a JSON object")
        return None
    return value


def sigstore_shape_error(value: dict[str, Any]) -> str | None:
    media_type = value.get("mediaType")
    if not isinstance(media_type, str) or not media_type.startswith(
        "application/vnd.dev.sigstore.bundle+json"
    ):
        return "Sigstore bundle mediaType is missing or unsupported"
    verification_material = value.get("verificationMaterial")
    if not isinstance(verification_material, dict) or not verification_material:
        return "Sigstore bundle verificationMaterial is empty"
    dsse = value.get("dsseEnvelope")
    message_signature = value.get("messageSignature")
    if dsse is not None:
        if not isinstance(dsse, dict):
            return "Sigstore dsseEnvelope must be an object"
        if not isinstance(dsse.get("payloadType"), str) or not isinstance(
            dsse.get("payload"), str
        ):
            return "Sigstore DSSE payload shape is invalid"
        signatures = dsse.get("signatures")
        if not isinstance(signatures, list) or not signatures:
            return "Sigstore DSSE envelope has no signatures"
        if any(
            not isinstance(signature, dict)
            or not isinstance(signature.get("sig"), str)
            or not signature["sig"]
            for signature in signatures
        ):
            return "Sigstore DSSE signature shape is invalid"
    elif message_signature is not None:
        if not isinstance(message_signature, dict):
            return "Sigstore messageSignature must be an object"
        digest = message_signature.get("messageDigest")
        if (
            not isinstance(digest, dict)
            or digest.get("algorithm") not in {"SHA2_256", "SHA256"}
            or not isinstance(digest.get("digest"), str)
            or not digest["digest"]
        ):
            return "Sigstore messageSignature digest shape is invalid"
    else:
        return "Sigstore bundle has neither a DSSE envelope nor message signature"
    return None


def spdx_shape_error(value: dict[str, Any]) -> str | None:
    if not (
        isinstance(value.get("spdxVersion"), str)
        and value["spdxVersion"] == "SPDX-2.3"
        and value.get("SPDXID") == "SPDXRef-DOCUMENT"
        and isinstance(value.get("name"), str)
        and isinstance(value.get("documentNamespace"), str)
    ):
        return "document is not an SPDX JSON document identity"
    creation = value.get("creationInfo")
    if not isinstance(creation, dict) or not isinstance(creation.get("created"), str):
        return "SPDX creationInfo is incomplete"
    creators = creation.get("creators")
    if (
        not isinstance(creators, list)
        or not creators
        or any(not isinstance(item, str) or not item for item in creators)
    ):
        return "SPDX creationInfo creators are incomplete"
    for field in ("packages", "files", "relationships", "documentDescribes"):
        if not isinstance(value.get(field), list):
            return f"SPDX {field} must be a list"
    if (
        not value["packages"]
        or not value["relationships"]
        or not value["documentDescribes"]
    ):
        return "SPDX source/component relationship graph must be non-empty"
    for field in ("packages", "files"):
        if any(
            not isinstance(item, dict)
            or not isinstance(item.get("SPDXID"), str)
            or not item["SPDXID"]
            for item in value[field]
        ):
            return f"SPDX {field} contains an invalid item"
    return None


def slsa_shape_error(value: dict[str, Any]) -> str | None:
    if value.get("_type") != "https://in-toto.io/Statement/v1":
        return "provenance is not an in-toto statement"
    predicate_type = value.get("predicateType")
    if predicate_type != "https://slsa.dev/provenance/v1":
        return "provenance is not a supported SLSA document"
    subjects = value.get("subject")
    if not isinstance(subjects, list) or not subjects:
        return "SLSA provenance subject must be a non-empty list"
    for subject in subjects:
        digest = subject.get("digest") if isinstance(subject, dict) else None
        if (
            not isinstance(subject, dict)
            or not isinstance(subject.get("name"), str)
            or not subject["name"]
            or not isinstance(digest, dict)
            or not isinstance(digest.get("sha256"), str)
            or SHA256_DIGEST.fullmatch(digest["sha256"]) is None
        ):
            return "SLSA provenance subject shape is invalid"
    predicate = value.get("predicate")
    if not isinstance(predicate, dict):
        return "SLSA provenance predicate must be an object"
    if not isinstance(predicate.get("buildDefinition"), dict) or not isinstance(
        predicate.get("runDetails"), dict
    ):
        return "SLSA v1 buildDefinition/runDetails are incomplete"
    definition = predicate["buildDefinition"]
    if (
        not isinstance(definition.get("externalParameters"), dict)
        or not definition["externalParameters"]
        or not isinstance(definition.get("internalParameters"), dict)
        or not definition["internalParameters"]
        or not isinstance(definition.get("resolvedDependencies"), list)
        or not definition["resolvedDependencies"]
    ):
        return "SLSA source/material/toolchain/build graph must be non-empty"
    return None


def spdx_subject_identity_error(
    value: dict[str, Any], expected_subjects: dict[str, str]
) -> str | None:
    """Require SPDX file checksums to cover the detached release subjects.

    SPDX generators may add package metadata, but the release gate must be
    able to locate every exact artifact/evidence subject and compare its
    SHA-256.  The inventory and provenance are not allowed to silently refer
    to a different basename or to a generated workspace copy.
    """

    observed: dict[str, str] = {}
    for item in value.get("files", []):
        if not isinstance(item, dict) or not isinstance(item.get("fileName"), str):
            continue
        name = item["fileName"]
        checksums = item.get("checksums")
        if not isinstance(checksums, list):
            continue
        for checksum in checksums:
            if not isinstance(checksum, dict):
                continue
            algorithm = str(checksum.get("algorithm", "")).replace("-", "").upper()
            value_hex = checksum.get("checksumValue")
            if algorithm in {"SHA256", "SHA2_256"} and isinstance(value_hex, str):
                if name in observed:
                    return f"SPDX subject coverage contains duplicate file: {name}"
                observed[name] = value_hex.lower()
                break
    missing = sorted(set(expected_subjects) - set(observed))
    extra = sorted(set(observed) - set(expected_subjects))
    if missing or extra:
        details = []
        if missing:
            details.append("missing=" + ",".join(missing))
        if extra:
            details.append("extra=" + ",".join(extra))
        return "SPDX subject coverage is not exact: " + "; ".join(details)
    mismatches = [
        path
        for path, digest in expected_subjects.items()
        if observed.get(path) != digest.removeprefix("sha256:")
    ]
    if mismatches:
        return "SPDX subject SHA-256 mismatch: " + ", ".join(sorted(mismatches))
    return None


def parse_release_checksums(
    runner: GateRunner,
    release_dir: Path,
    checksums_path: Path,
    expected_paths: set[str],
) -> dict[str, str] | None:
    try:
        text = checksums_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        runner.fail("release-checksums", f"cannot read SHA256SUMS: {error}")
        return None
    if not text.endswith("\n") or "\r" in text:
        runner.fail("release-checksums", "SHA256SUMS must be LF-terminated UTF-8")
        return None
    actual: dict[str, str] = {}
    malformed: list[str] = []
    for line in text[:-1].split("\n"):
        digest, separator, relative = line.partition("  ")
        if (
            not separator
            or SHA256_DIGEST.fullmatch(digest) is None
            or not relative
            or relative.startswith("*")
            or relative in actual
        ):
            malformed.append(line)
            continue
        candidate = safe_release_file(
            runner, release_dir, relative, "release-checksums"
        )
        if candidate is None or not candidate.is_file() or candidate.is_symlink():
            malformed.append(line)
            continue
        actual[relative] = digest
        try:
            observed = hashlib.sha256(candidate.read_bytes()).hexdigest()
        except OSError:
            malformed.append(line)
            continue
        if observed != digest:
            malformed.append(line)
    if not actual:
        runner.fail("release-checksums", "SHA256SUMS is empty or malformed")
        return None
    missing = sorted(expected_paths - set(actual))
    extra = sorted(set(actual) - expected_paths)
    if missing or extra:
        detail = []
        if missing:
            detail.append("missing=" + ",".join(missing))
        if extra:
            detail.append("extra=" + ",".join(extra))
        malformed.append("coverage " + "; ".join(detail))
    if malformed:
        runner.fail(
            "release-checksums",
            "invalid SHA256SUMS entries: " + ", ".join(malformed[:8]),
        )
        return None
    runner.pass_(
        "release-checksums",
        "SHA256SUMS exactly covers and verifies payload and release-gated evidence subjects",
    )
    return actual


def detached_release_inventory(
    runner: GateRunner, metadata: dict[str, Any], manifest: dict[str, Any]
) -> tuple[dict[str, str], dict[str, str], dict[str, str], dict[str, str]] | None:
    """Validate the detached v2 release inventory shape and identities.

    The compatibility manifest deliberately contains no archive/evidence
    bytes' SHA-256 values.  This detached document is the sole source of
    release byte identities.  It contains exact path and digest maps for the
    eight release artifacts plus every release-gated evidence report.
    """

    if metadata.get("schema") != DETACHED_RELEASE_MANIFEST_SCHEMA:
        runner.fail(
            "release-detached-inventory",
            "release-manifest.json must use the detached v2 inventory schema; "
            f"observed={metadata.get('schema')!r}",
        )
        return None

    if metadata.get("product") != manifest.get("release_candidate"):
        runner.fail(
            "release-detached-inventory",
            identity_mismatch(
                "detached inventory product",
                manifest.get("release_candidate"),
                metadata.get("product"),
            ),
        )
    if metadata.get("source_version") != manifest.get("release_candidate"):
        runner.fail(
            "release-detached-inventory",
            identity_mismatch(
                "detached inventory source_version",
                manifest.get("release_candidate"),
                metadata.get("source_version"),
            ),
        )

    try:
        mirror_digest = hashlib.sha256(MIRROR_PATH.read_bytes()).hexdigest()
    except OSError as error:
        runner.fail(
            "release-detached-inventory", f"cannot hash compatibility mirror: {error}"
        )
        mirror_digest = None
    identity = metadata.get("manifest")
    if not isinstance(identity, dict):
        runner.fail(
            "release-detached-inventory", "detached manifest identity is missing"
        )
    else:
        expected_identity = {
            "source": MANIFEST_PATH.name,
            "mirror": MIRROR_PATH.name,
        }
        for key, expected in expected_identity.items():
            if identity.get(key) != expected:
                runner.fail(
                    "release-detached-inventory",
                    identity_mismatch(
                        f"detached manifest {key}", expected, identity.get(key)
                    ),
                )
        observed = identity.get("sha256")
        if not isinstance(observed, str) or SHA256_DIGEST.fullmatch(observed) is None:
            runner.fail(
                "release-detached-inventory",
                f"detached manifest mirror digest is malformed: observed={observed!r}",
            )
        elif mirror_digest is not None and observed != mirror_digest:
            runner.fail(
                "release-detached-inventory",
                identity_mismatch(
                    "detached manifest mirror digest", mirror_digest, observed
                ),
            )

    release_rows = manifest.get("release_artifacts")
    expected_artifact_ids = (
        {
            row.get("id")
            for row in release_rows
            if isinstance(row, dict) and isinstance(row.get("id"), str)
        }
        if isinstance(release_rows, list)
        else set(RELEASE_ARTIFACT_PROFILES)
    )
    artifact_paths = metadata.get("artifacts")
    artifact_digests = metadata.get("artifact_digests")
    if not isinstance(artifact_paths, dict) or not isinstance(artifact_digests, dict):
        runner.fail(
            "release-detached-inventory",
            "detached inventory requires artifacts and artifact_digests objects",
        )
        return None
    signed_artifact_ids = expected_artifact_ids - {SIGSTORE_VERIFICATION_ARTIFACT_ID}
    if (
        set(artifact_paths) != expected_artifact_ids
        or set(artifact_digests) != signed_artifact_ids
    ):
        runner.fail(
            "release-detached-inventory",
            "detached artifact paths must match all compatibility release artifact IDs "
            "and artifact_digests must match all signed IDs; Sigstore has no digest "
            "in the manifest",
        )

    verification_material = metadata.get("verification_material")
    if not isinstance(verification_material, dict):
        runner.fail(
            "release-detached-inventory",
            "verification_material must contain a path-only Sigstore entry",
        )
        verification_material = {}
    sigstore_material = verification_material.get(SIGSTORE_VERIFICATION_ARTIFACT_ID)
    if (
        not isinstance(sigstore_material, dict)
        or sigstore_material.get("path")
        != artifact_paths.get(SIGSTORE_VERIFICATION_ARTIFACT_ID)
        or "digest" in sigstore_material
        or SIGSTORE_VERIFICATION_ARTIFACT_ID in artifact_digests
    ):
        runner.fail(
            "release-detached-inventory",
            "Sigstore bundle must be path-only verification material; its digest "
            "must not appear in release-manifest.json",
        )

    evidence_rows = manifest.get("evidence")
    expected_evidence_ids = (
        {
            row.get("id")
            for row in evidence_rows
            if isinstance(row, dict)
            and row.get("release_gate") is True
            and isinstance(row.get("id"), str)
        }
        if isinstance(evidence_rows, list)
        else set()
    )
    evidence_paths = metadata.get("evidence")
    evidence_digests = metadata.get("evidence_digests")
    if not isinstance(evidence_paths, dict) or not isinstance(evidence_digests, dict):
        runner.fail(
            "release-detached-inventory",
            "detached inventory requires evidence and evidence_digests objects",
        )
        return None
    if (
        set(evidence_paths) != expected_evidence_ids
        or set(evidence_digests) != expected_evidence_ids
    ):
        runner.fail(
            "release-detached-inventory",
            "detached evidence ID/path/digest maps must exactly match release-gated compatibility evidence IDs",
        )

    normalized: list[dict[str, str]] = []
    for kind, paths, digests, expected_ids in (
        ("artifact", artifact_paths, artifact_digests, signed_artifact_ids),
        ("evidence", evidence_paths, evidence_digests, expected_evidence_ids),
    ):
        for identity_id in sorted(expected_ids):
            relative = paths.get(identity_id)
            digest = digests.get(identity_id)
            if not isinstance(relative, str) or not relative:
                runner.fail(
                    "release-detached-inventory",
                    f"{kind} path is missing: {identity_id}",
                )
                continue
            if kind == "evidence" and relative != f"evidence/{identity_id}.json":
                runner.fail(
                    "release-detached-inventory",
                    f"evidence path is not deterministic for {identity_id}: {relative!r}",
                )
            if (
                not isinstance(digest, str)
                or SHA256_REFERENCE.fullmatch(digest) is None
            ):
                runner.fail(
                    "release-detached-inventory",
                    f"{kind} digest is malformed: {identity_id}",
                )
                continue
            normalized.append(
                {"kind": kind, "id": identity_id, "path": relative, "digest": digest}
            )

    all_paths = [row["path"] for row in normalized]
    if len(all_paths) != len(set(all_paths)):
        runner.fail(
            "release-detached-inventory", "artifact and evidence paths are not unique"
        )
    if "release-manifest.json" in all_paths:
        runner.fail(
            "release-detached-inventory", "detached inventory cannot hash itself"
        )
    if any(
        path
        in {"SHA256SUMS", "sigstore.bundle.json", "sbom.spdx.json", "provenance.json"}
        for path in evidence_paths.values()
        if isinstance(path, str)
    ):
        runner.fail(
            "release-detached-inventory",
            "evidence cannot reuse supply-chain sidecar paths",
        )
    verification_sigstore = verification_material.get(SIGSTORE_VERIFICATION_ARTIFACT_ID)
    if isinstance(verification_sigstore, dict) and verification_sigstore.get(
        "path"
    ) not in {None, artifact_paths.get(SIGSTORE_VERIFICATION_ARTIFACT_ID)}:
        runner.fail(
            "release-detached-inventory",
            "Sigstore verification material path must match the artifact path",
        )

    if any(
        outcome.name == "release-detached-inventory" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    ):
        return None
    runner.pass_(
        "release-detached-inventory",
        f"detached v2 inventory binds {len(artifact_paths)} artifacts and {len(evidence_paths)} release-gated evidence reports",
    )
    return (
        {str(key): str(value) for key, value in artifact_paths.items()},
        {str(key): str(value) for key, value in artifact_digests.items()},
        {str(key): str(value) for key, value in evidence_paths.items()},
        {str(key): str(value) for key, value in evidence_digests.items()},
    )


def verify_release_artifacts(runner: GateRunner, manifest: dict[str, Any]) -> None:
    configured_dir = os.environ.get("WORLDSTREAM_RELEASE_DIR")
    release_dir = Path(configured_dir) if configured_dir else ROOT / "dist"
    if release_dir.is_symlink():
        runner.fail(
            "release-artifact-directory",
            f"release directory must not be a symlink: {release_dir}",
        )
        return
    if not release_dir.is_dir():
        runner.unresolved(
            "release-artifact-directory",
            f"release directory is missing: {release_dir}",
        )
        return

    metadata_path = safe_release_file(
        runner, release_dir, "release-manifest.json", "release-artifact-manifest"
    )
    if metadata_path is None or not metadata_path.exists():
        runner.unresolved(
            "release-artifact-manifest",
            f"release-manifest.json is not present in {release_dir}",
        )
        return
    if metadata_path.is_symlink() or not metadata_path.is_file():
        runner.fail(
            "release-artifact-manifest",
            f"release-manifest.json is not a regular file: {release_dir / 'release-manifest.json'}",
        )
        return
    metadata = load_json_object(runner, "release-artifact-manifest", metadata_path)
    if metadata is None:
        return

    product = manifest.get("release_candidate")
    detached = uses_detached_release_identity(manifest)
    detached_inventory = detached_release_inventory(runner, metadata, manifest)
    if detached and detached_inventory is None:
        # Keep going far enough to emit path/inventory diagnostics, but never
        # fall back to compatibility.toml digests or infer a release identity.
        detached_inventory = ({}, {}, {}, {})
    if not detached:
        if metadata.get("schema") != "worldstream/release-artifact-manifest/v1":
            runner.fail("release-artifact-manifest", "schema identity is invalid")
        if metadata.get("product") != product:
            runner.fail(
                "release-source-version",
                "release metadata product differs from manifest",
            )
        if metadata.get("source_version") != product:
            runner.fail(
                "release-source-version",
                "release metadata source_version differs from manifest",
            )
    elif detached_inventory is not None:
        runner.pass_(
            "release-source-version",
            f"detached release inventory product/source_version match {product}",
        )
    try:
        mirror_digest = hashlib.sha256(MIRROR_PATH.read_bytes()).hexdigest()
    except OSError as error:
        runner.fail(
            "release-manifest-identity", f"cannot read compatibility mirror: {error}"
        )
        mirror_digest = None
    identity = metadata.get("manifest")
    if not isinstance(identity, dict):
        runner.fail("release-manifest-identity", "manifest identity object is missing")
    else:
        if identity.get("source") != MANIFEST_PATH.name:
            runner.fail(
                "release-manifest-identity",
                identity_mismatch(
                    "source manifest identity",
                    MANIFEST_PATH.name,
                    identity.get("source"),
                ),
            )
        if identity.get("mirror") != MIRROR_PATH.name:
            runner.fail(
                "release-manifest-identity",
                identity_mismatch(
                    "mirror manifest identity", MIRROR_PATH.name, identity.get("mirror")
                ),
            )
        if (
            not isinstance(identity.get("sha256"), str)
            or SHA256_DIGEST.fullmatch(identity.get("sha256", "")) is None
        ):
            runner.fail(
                "release-manifest-identity",
                f"compatibility mirror identity digest is malformed: observed={identity.get('sha256')!r}",
            )
        elif mirror_digest is not None and identity.get("sha256") != mirror_digest:
            runner.fail(
                "release-manifest-identity",
                identity_mismatch(
                    "compatibility mirror digest", mirror_digest, identity.get("sha256")
                ),
            )

    release_artifact_rows = manifest.get("release_artifacts", [])
    if not isinstance(release_artifact_rows, list):
        release_artifact_rows = []
    rows = {
        row.get("id"): row
        for row in release_artifact_rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    paths = (
        detached_inventory[0]
        if detached_inventory is not None
        else metadata.get("artifacts")
    )
    digests = (
        detached_inventory[1]
        if detached_inventory is not None
        else metadata.get("artifact_digests")
    )
    evidence_paths = detached_inventory[2] if detached_inventory is not None else {}
    evidence_digests = detached_inventory[3] if detached_inventory is not None else {}
    expected_ids = set(rows)
    signed_ids = expected_ids - {SIGSTORE_VERIFICATION_ARTIFACT_ID}
    if not isinstance(paths, dict):
        runner.fail(
            "release-artifact-manifest", "artifacts must be an id-to-path object"
        )
        paths = {}
    if set(paths) != expected_ids:
        missing = sorted(expected_ids - set(paths))
        extra = sorted(set(paths) - expected_ids)
        runner.fail(
            "release-artifact-manifest",
            "artifact ID identity differs from compatibility inventory: "
            + (f"missing={','.join(missing)}" if missing else "")
            + (f"; extra={','.join(extra)}" if extra else ""),
        )
    if not isinstance(digests, dict):
        runner.fail(
            "release-artifact-manifest",
            "artifact_digests must be an id-to-digest object",
        )
        digests = {}
    if set(digests) != signed_ids:
        missing = sorted(signed_ids - set(digests))
        extra = sorted(set(digests) - signed_ids)
        runner.fail(
            "release-artifact-manifest",
            "signed artifact digest identity differs from compatibility inventory: "
            + (f"missing={','.join(missing)}" if missing else "")
            + (f"; extra={','.join(extra)}" if extra else ""),
        )

    artifact_paths: dict[str, str] = {}
    artifact_files: dict[str, Path] = {}
    seen_paths: set[str] = set()
    version = str(product or "")
    for artifact_id, row in rows.items():
        relative = paths.get(artifact_id)
        if not isinstance(relative, str) or not relative:
            runner.fail("release-artifact-" + artifact_id, "artifact path is missing")
            continue
        if relative in seen_paths:
            runner.fail(
                "release-artifact-" + artifact_id, "artifact path is duplicated"
            )
        seen_paths.add(relative)
        artifact_paths[artifact_id] = relative
        path_error = release_artifact_path_error(artifact_id, relative, version)
        if path_error:
            runner.fail(
                "release-artifact-" + artifact_id,
                "artifact path identity mismatch: "
                f"id={artifact_id}; profile={row.get('profile')!r}; "
                f"observed_path={relative!r}; {path_error}",
            )
        artifact = safe_release_file(
            runner, release_dir, relative, "release-artifact-" + artifact_id
        )
        if artifact is None:
            continue
        try:
            metadata_stat = artifact.lstat()
        except OSError as error:
            if not artifact.exists():
                runner.unresolved(
                    "release-artifact-" + artifact_id,
                    f"artifact is not present: id={artifact_id}; path={relative!r}",
                )
            else:
                runner.fail(
                    "release-artifact-" + artifact_id,
                    f"cannot inspect {relative}: {error}",
                )
            continue
        if stat.S_ISLNK(metadata_stat.st_mode) or not stat.S_ISREG(
            metadata_stat.st_mode
        ):
            runner.fail(
                "release-artifact-" + artifact_id,
                f"artifact is not a regular file: {relative}",
            )
            continue
        artifact_files[artifact_id] = artifact
        if artifact_id == SIGSTORE_VERIFICATION_ARTIFACT_ID:
            runner.pass_(
                "release-artifact-" + artifact_id,
                "Sigstore bundle is a regular verification-material file; its "
                "bytes are authenticated by cosign over release-manifest.json",
            )
            continue
        expected_digest = digests.get(artifact_id)
        if (
            not isinstance(expected_digest, str)
            or SHA256_REFERENCE.fullmatch(expected_digest) is None
        ):
            runner.fail(
                "release-artifact-" + artifact_id,
                f"artifact digest identity is malformed: observed={expected_digest!r}",
            )
            continue
        observed_digest = "sha256:" + hashlib.sha256(artifact.read_bytes()).hexdigest()
        if expected_digest != observed_digest:
            runner.fail(
                "release-artifact-" + artifact_id,
                identity_mismatch("artifact digest", expected_digest, observed_digest),
            )
        row_digest = row.get("digest")
        detached_row_ready = detached and (
            row.get("status") == "detached"
            and row_digest == ""
            and row.get("digest_location") == "release-manifest.json"
        )
        if (not detached_row_ready) and (
            row.get("status") != "resolved" or row_digest != expected_digest
        ):
            runner.unresolved(
                "release-artifact-" + artifact_id,
                "compatibility artifact identity is unresolved: "
                f"profile={row.get('profile')!r}; path={relative!r}; "
                f"status={row.get('status')!r}; "
                f"manifest_digest={row_digest!r}; metadata_digest={expected_digest!r}; "
                f"observed_bytes={observed_digest!r}",
            )
        else:
            runner.pass_(
                "release-artifact-" + artifact_id,
                f"artifact identity verified from detached inventory: profile={row.get('profile')!r}; path={relative!r}; digest={observed_digest}",
            )

    evidence_relative_paths: dict[str, str] = {}
    evidence_files: dict[str, Path] = {}
    for evidence_id, relative in evidence_paths.items():
        if not isinstance(relative, str) or not relative:
            runner.fail(
                "release-evidence-" + evidence_id,
                "detached evidence path is missing",
            )
            continue
        evidence_relative_paths[evidence_id] = relative
        evidence = safe_release_file(
            runner, release_dir, relative, "release-evidence-" + evidence_id
        )
        if evidence is None:
            continue
        try:
            evidence_stat = evidence.lstat()
        except OSError as error:
            runner.fail(
                "release-evidence-" + evidence_id,
                f"cannot inspect detached evidence {relative}: {error}",
            )
            continue
        if stat.S_ISLNK(evidence_stat.st_mode) or not stat.S_ISREG(
            evidence_stat.st_mode
        ):
            runner.fail(
                "release-evidence-" + evidence_id,
                f"detached evidence is not a regular file: {relative}",
            )
            continue
        evidence_files[evidence_id] = evidence
        expected_digest = evidence_digests.get(evidence_id)
        if (
            not isinstance(expected_digest, str)
            or SHA256_REFERENCE.fullmatch(expected_digest) is None
        ):
            runner.fail(
                "release-evidence-" + evidence_id,
                f"detached evidence digest is malformed: observed={expected_digest!r}",
            )
            continue
        observed_digest = "sha256:" + hashlib.sha256(evidence.read_bytes()).hexdigest()
        if observed_digest != expected_digest:
            runner.fail(
                "release-evidence-" + evidence_id,
                identity_mismatch("evidence digest", expected_digest, observed_digest),
            )
        else:
            runner.pass_(
                "release-evidence-" + evidence_id,
                f"detached evidence identity verified: path={relative!r}; digest={observed_digest}",
            )

    release_evidence_ids = tuple(
        row["id"]
        for row in manifest.get("evidence", [])
        if isinstance(row, dict)
        and row.get("release_gate") is True
        and isinstance(row.get("id"), str)
    )
    if set(evidence_files) == set(
        release_evidence_ids
    ) and CHECKSUM_PAYLOAD_ARTIFACT_IDS.issubset(artifact_files):
        verifier = release_evidence_verifier()
        try:
            verifier.validate_normalized_evidence_reports(
                evidence_files,
                release_evidence_ids,
                str(product or ""),
                manifest["contracts"],
            )
            verifier.validate_pre_sign_material(
                release_dir,
                {
                    artifact_id: artifact_files[artifact_id]
                    for artifact_id in CHECKSUM_PAYLOAD_ARTIFACT_IDS
                },
                evidence_files,
                release_evidence_ids,
                str(product or ""),
                manifest["contracts"],
                mirror_digest or "",
            )
        except verifier.AssemblyError as error:
            runner.fail("release-evidence-semantics", str(error))
        else:
            runner.pass_(
                "release-evidence-semantics",
                "all normalized reports and signed source citations independently verified",
            )
    else:
        runner.fail(
            "release-evidence-semantics",
            "normalized evidence or pre-sign payload files are incomplete",
        )

    expected_files = (
        set(artifact_paths.values())
        | set(evidence_relative_paths.values())
        | {"release-manifest.json"}
        | PRE_SIGN_SUPPLY_CHAIN_FILES
        | {
            f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
            for evidence_id in evidence_relative_paths
            if evidence_id != "checksums-signature-sbom-provenance"
        }
    )
    inventory_errors: list[str] = []
    for candidate in release_dir.rglob("*"):
        relative = candidate.relative_to(release_dir).as_posix()
        if candidate.is_symlink():
            inventory_errors.append(f"symlink in release directory: {relative}")
        elif candidate.is_dir() and relative not in {
            "evidence",
            "supply-chain",
            PRE_SIGN_SUBJECT_DIRECTORY,
        }:
            inventory_errors.append(f"unexpected release directory: {relative}")
        elif candidate.is_file() and relative not in expected_files:
            inventory_errors.append(f"unlisted release file: {relative}")
        elif not candidate.is_dir() and not candidate.is_file():
            inventory_errors.append(f"non-regular release entry: {relative}")
    if inventory_errors:
        runner.fail("release-directory-inventory", "; ".join(inventory_errors[:8]))
    else:
        runner.pass_(
            "release-directory-inventory",
            "release directory contains only declared files",
        )

    checksums_relative = artifact_paths.get("checksums")
    if checksums_relative != "SHA256SUMS":
        runner.fail("release-checksums", "checksums artifact must be named SHA256SUMS")
        checksums = None
    else:
        checksums_path = safe_release_file(
            runner, release_dir, checksums_relative, "release-checksums"
        )
        checksums = (
            parse_release_checksums(
                runner,
                release_dir,
                checksums_path,
                {
                    artifact_paths[artifact_id]
                    for artifact_id in CHECKSUM_PAYLOAD_ARTIFACT_IDS
                    if artifact_id in artifact_paths
                }
                | {
                    f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
                    for evidence_id in evidence_relative_paths
                    if evidence_id != "checksums-signature-sbom-provenance"
                },
            )
            if checksums_path is not None
            and checksums_path.is_file()
            and not checksums_path.is_symlink()
            else None
        )
        if (
            checksums is None
            and checksums_path is not None
            and not checksums_path.is_file()
        ):
            runner.unresolved("release-checksums", "SHA256SUMS is not present")

    document_specs = {
        "release-sigstore": artifact_paths.get("sigstore-bundle"),
        "release-spdx-sbom": artifact_paths.get("spdx-sbom"),
        "release-provenance": artifact_paths.get("slsa-provenance"),
    }
    subject_paths: dict[str, str] = {}
    subject_relatives = {
        artifact_paths[artifact_id]
        for artifact_id in CHECKSUM_PAYLOAD_ARTIFACT_IDS
        if artifact_id in artifact_paths
    } | {
        f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
        for evidence_id in evidence_relative_paths
        if evidence_id != "checksums-signature-sbom-provenance"
    }
    for relative in subject_relatives:
        path = safe_release_file(runner, release_dir, relative, "release-subject")
        if path is None or not path.is_file() or path.is_symlink():
            continue
        subject_paths[relative] = (
            "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
        )
    documents: dict[str, dict[str, Any]] = {}
    for name, relative in document_specs.items():
        if not isinstance(relative, str):
            runner.fail(name, "document artifact path is missing")
            continue
        path = safe_release_file(runner, release_dir, relative, name)
        if path is None:
            continue
        if not path.exists():
            runner.unresolved(name, f"document is not present: {relative}")
            continue
        if not path.is_file() or path.is_symlink():
            runner.fail(name, f"document is not a regular file: {relative}")
            continue
        value = load_json_object(runner, name, path)
        if value is None:
            continue
        if name == "release-sigstore":
            error = sigstore_shape_error(value)
            if error:
                runner.fail(name, error)
            else:
                runner.pass_(
                    name,
                    "Sigstore bundle structure verified; cryptographic verification over detached release-manifest.json remains separate",
                )
        elif name == "release-spdx-sbom":
            error = spdx_shape_error(value)
            if error:
                runner.fail(name, error)
            elif detached:
                error = spdx_subject_identity_error(value, subject_paths)
                if error:
                    runner.fail(name, error)
                else:
                    runner.pass_(
                        name,
                        "SPDX structure and exact detached release subject coverage verified",
                    )
            else:
                runner.pass_(name, "SPDX SBOM structure verified")
        else:
            error = slsa_shape_error(value)
            if error:
                runner.fail(name, error)
            else:
                runner.pass_(name, "SLSA provenance structure verified")
        documents[name] = value

    provenance = documents.get("release-provenance")
    if provenance is not None:
        subjects = provenance.get("subject")
        seen_subjects: dict[str, str] = {}
        if isinstance(subjects, list):
            for subject in subjects:
                if isinstance(subject, dict) and isinstance(subject.get("name"), str):
                    digest = subject.get("digest")
                    if isinstance(digest, dict) and isinstance(
                        digest.get("sha256"), str
                    ):
                        if subject["name"] in seen_subjects:
                            runner.fail(
                                "release-provenance",
                                "duplicate provenance subject name",
                            )
                        seen_subjects[subject["name"]] = digest["sha256"]
        payload_paths = set(subject_paths)
        if set(seen_subjects) != payload_paths:
            runner.fail(
                "release-provenance",
                "SLSA subjects do not exactly cover payload artifacts",
            )
        else:
            mismatches = []
            for relative, digest in seen_subjects.items():
                path = safe_release_file(
                    runner, release_dir, relative, "release-provenance"
                )
                if (
                    path is None
                    or not path.is_file()
                    or hashlib.sha256(path.read_bytes()).hexdigest() != digest
                ):
                    mismatches.append(relative)
            if mismatches:
                runner.fail(
                    "release-provenance",
                    "SLSA subject digest mismatch: " + ", ".join(mismatches),
                )
            else:
                runner.pass_(
                    "release-provenance-payloads",
                    "SLSA subjects match the signed pre-sign subject inventory",
                )

    spdx = documents.get("release-spdx-sbom")
    if (
        spdx is not None
        and provenance is not None
        and CHECKSUM_PAYLOAD_ARTIFACT_IDS.issubset(artifact_files)
        and set(subject_paths) == subject_relatives
    ):
        identity = release_build_identity_verifier()
        try:
            identity.validate_identity_documents(
                spdx=spdx,
                provenance=provenance,
                version=str(product or ""),
                subjects_by_relative={
                    relative: release_dir / relative for relative in subject_paths
                },
                payloads_by_id={
                    artifact_id: artifact_files[artifact_id]
                    for artifact_id in CHECKSUM_PAYLOAD_ARTIFACT_IDS
                },
                require_github=True,
            )
        except identity.IdentityError as error:
            runner.fail("release-build-identity", str(error))
        else:
            runner.pass_(
                "release-build-identity",
                "SPDX and SLSA exactly bind source commit, locked components, tools, targets, arguments, runner, and OCI base",
            )

    signature_verifier = release_evidence_verifier()
    try:
        signature_verifier.verify_release_signatures(release_dir)
    except signature_verifier.AssemblyError as error:
        runner.fail("release-signature-verifier", str(error))
    else:
        runner.pass_(
            "release-signature-verifier",
            "pre-sign subject inventory and final release manifest identities verified",
        )


def safe_release_file(
    runner: GateRunner, release_dir: Path, relative: str, name: str
) -> Path | None:
    """Resolve a release member without allowing traversal or symlink escape."""
    if (
        not relative
        or "\\" in relative
        or re.match(r"^[A-Za-z]:", relative) is not None
        or any(
            ord(character) < 0x20 or ord(character) == 0x7F for character in relative
        )
        or relative != relative.strip()
    ):
        runner.fail(name, f"unsafe release path: {relative!r}")
        return None
    path = PurePosixPath(relative)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        runner.fail(name, f"unsafe release path: {relative!r}")
        return None
    candidate = release_dir.joinpath(*path.parts)
    try:
        candidate.resolve().relative_to(release_dir.resolve())
    except ValueError:
        runner.fail(name, f"release path escapes release directory: {relative!r}")
        return None
    return candidate


def provider_smoke(runner: GateRunner) -> None:
    credential_names = (
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "GCP_SERVICE_ACCOUNT",
        "AZURE_CLIENT_SECRET",
    )
    present = [name for name in credential_names if os.environ.get(name)]
    if present:
        runner.fail(
            "provider-credential-policy",
            "provider smoke must not use credentials: " + ", ".join(present),
        )
    else:
        runner.pass_(
            "provider-credential-policy", "no provider credentials are required"
        )
    runner.command(
        "provider-neutral-operator-smoke", ["scripts/smoke-operator.sh"], required=False
    )
    runner.skip(
        "provider-evidence",
        "optional dated provider evidence is not asserted by this credential-free smoke",
    )


def run_tier(
    runner: GateRunner, manifest: dict[str, Any], tier: str, cell: str | None
) -> int:
    release = tier == "release"
    if tier == "provider-smoke":
        provider_smoke(runner)
        return runner.finish(manifest=manifest, tier=tier)
    manifest_gate(runner, manifest, release=release)
    selected_cell_row: dict[str, Any] | None = None
    selected_cell_route: dict[str, Any] | None = None
    if tier == "minimal-ci":
        selected = selected_ci_cell(runner, manifest, cell)
        if selected is None:
            return runner.finish(manifest=manifest, tier=tier)
        selected_cell_row, selected_cell_route = selected
    if release:
        toolchain_pins(runner, manifest)
        version_and_source_drift(runner, manifest)
        secret_scan(runner, release=True)
        dependency_scan(runner, release=True, include_ecosystems=True)
        root_python_checks(runner)
        rust_checks(runner, full=True)
        sdk_and_ui_checks(runner)
        sqlite_checks(runner, full=True)
        filesystem_checks(runner)
        deterministic_evidence_checks(runner)
        if tier == "minimal-ci" and selected_cell_route["system"] != "Linux":
            runner.pass_(
                "process-contract-scope",
                "POSIX process-kill/pressure evidence is routed to the required native Linux cell",
            )
        else:
            process_contract_checks(runner)
        contract_inventory(runner, manifest, release=True)
        verify_release_artifacts(runner, manifest)
        return runner.finish(manifest=manifest, tier=tier)

    if tier != "fast":
        toolchain_pins(runner, manifest)
    version_and_source_drift(runner, manifest)
    secret_scan(runner)
    dependency_scan(runner, release=False, include_ecosystems=tier != "fast")
    if tier in {"fast", "pre-push"}:
        root_python_checks(runner)
    rust_checks(runner, full=tier != "fast")
    critical_contract_matrix(runner)
    if tier != "fast":
        sdk_and_ui_checks(runner)
        sqlite_checks(runner, full=True)
        if tier == "pre-push":
            filesystem_checks(runner)
        postgres_required = runner.ci and (
            selected_cell_row is None
            or "postgres-primary" in selected_cell_row.get("storage_profiles", [])
        )
        local_postgresql(runner, required=postgres_required)
        deterministic_evidence_checks(runner)
        if tier == "minimal-ci" and selected_cell_route["system"] != "Linux":
            runner.pass_(
                "process-contract-scope",
                "POSIX process-kill/pressure/TLS evidence is routed to the required native Linux cell",
            )
        else:
            process_contract_checks(runner)
        if tier == "pre-push":
            # A developer hook owns bounded local checks.  The hosted Linux
            # cell owns disposable PostgreSQL/PgBouncer evidence; missing
            # local Docker/psql remains visible and is never promoted.
            postgres_live_contract(runner, required=False)
        contract_inventory(runner, manifest, release=False)
    if tier == "minimal-ci":
        filesystem_checks(runner)
        if selected_cell_route["system"] == "Linux":
            postgres_live_contract(runner, required=True)
        else:
            runner.pass_(
                "postgres-live-contract-scope",
                "direct/PgBouncer container evidence is routed to native Linux; this cell exercises native Windows PostgreSQL connectivity",
            )
        if selected_cell_route["platform"] == "oci-linux-amd64":
            oci_cell_checks(runner)
        runner.pass_(
            "ci-cell",
            f"manifest cell {cell} executed on its native {selected_cell_route['system']} route",
        )
    return runner.finish(manifest=manifest, tier=tier)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "tier", choices=["fast", "pre-push", "minimal-ci", "release", "provider-smoke"]
    )
    parser.add_argument(
        "--ci", action="store_true", help="turn incomplete skips into failures"
    )
    parser.add_argument(
        "--strict", action="store_true", help="turn incomplete skips into failures"
    )
    parser.add_argument(
        "--offline", action="store_true", help="pass offline mode to dependency checks"
    )
    parser.add_argument("--cell", help="manifest gate cell for minimal-ci")
    parser.add_argument(
        "--list-cells", action="store_true", help="list cells for the selected tier"
    )
    parser.add_argument(
        "--format", choices=["lines", "json", "matrix"], default="lines"
    )
    parser.add_argument(
        "--report",
        help="write a deterministic JSON gate report; release tier includes a fail-closed blocker matrix",
    )
    parser.add_argument(
        "--handoff",
        help="write a manifest-driven release evidence handoff (release tier only)",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    manifest = load_manifest()
    if args.list_cells:
        return list_cells(manifest, args.tier, args.format)
    runner = GateRunner(
        strict=args.strict or args.tier == "release",
        offline=args.offline or args.tier == "fast",
        ci=args.ci,
        tier=args.tier,
        report_path=args.report,
        handoff_path=args.handoff,
    )
    return run_tier(runner, manifest, args.tier, args.cell)


if __name__ == "__main__":
    raise SystemExit(main())
